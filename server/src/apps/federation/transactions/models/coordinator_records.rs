//! Coordinator changes and their audit history commit in one ORM transaction.

use super::states::{AtomicCoordinatorDecision, AtomicHistoryRole, AtomicVotePhase};
use super::{AtomicCoordinator, AtomicHistory, AtomicVote};
use crate::apps::federation::transactions::serializers::{
	contracts::{Manifest, Status, Vote},
	protocol::TransactionHistory,
};
use crate::apps::federation::transactions::services::decisions::CoordinatorTransition;
use crate::{Error, Result};
use chrono::Utc;
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::{DatabaseConnection as BackendConnection, TransactionExecutor};
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{DatabaseConnection, Model, OrmExecutor, QueryRow};
use reinhardt::query::{
	Alias, Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::json;
use uuid::Uuid;

pub async fn transition(
	connection: DatabaseConnection,
	id: Uuid,
	change: CoordinatorTransition,
	detail: &str,
) -> Result<bool> {
	connection
		.atomic(async |tx| transition_in(tx, id, change, detail).await)
		.await
}

pub(crate) async fn transition_in(
	tx: &mut dyn TransactionExecutor,
	id: Uuid,
	change: CoordinatorTransition,
	detail: &str,
) -> Result<bool> {
	let mut state = AtomicCoordinator::objects()
		.filter(AtomicCoordinator::field_id().eq(id))
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?
		.pop()
		.ok_or_else(|| Error::NotFound("tx".into()))?;
	if !change.apply(&mut state)? {
		return Ok(false);
	}
	state.last_error = (!detail.is_empty() && matches!(change, CoordinatorTransition::Decide(_)))
		.then(|| detail.to_owned());
	AtomicCoordinator::objects()
		.save_with_executor(tx, &state)
		.await
		.map_err(FrameworkError::from)?;
	AtomicHistory::append(tx, id, "coordinator", change.phase(), detail).await?;
	Ok(true)
}

fn contract(state: AtomicCoordinator) -> Status {
	Status {
		id: state.id,
		digest: state.digest,
		manifest: state.manifest.0,
		decision: state.decision.map(|decision| match decision {
			AtomicCoordinatorDecision::Commit => "COMMIT".to_owned(),
			AtomicCoordinatorDecision::Abort => "ABORT".to_owned(),
		}),
		visible: state.visible,
		complete: state.complete,
		last_error: state.last_error,
		created_at: state.created_at,
	}
}

pub(crate) async fn status<E: OrmExecutor>(db: &mut E, id: Uuid) -> Result<Status> {
	AtomicCoordinator::objects()
		.filter(AtomicCoordinator::field_id().eq(id))
		.first_with_db(db)
		.await?
		.map(contract)
		.ok_or_else(|| Error::NotFound("transaction".into()))
}

pub(crate) async fn votes<E: OrmExecutor>(db: &mut E, id: Uuid) -> Result<Vec<Vote>> {
	Ok(AtomicVote::objects()
		.filter(AtomicVote::field_transaction_id().eq(id))
		.order_by(&["node_id"])
		.all_with_db(db)
		.await?
		.into_iter()
		.map(|vote| Vote {
			node_id: vote.node_id,
			phase: vote.phase.as_str().to_owned(),
		})
		.collect())
}

pub(crate) async fn admit_in(
	tx: &mut dyn TransactionExecutor,
	manifest: &Manifest,
) -> Result<Status> {
	let (sql, values) = Query::select()
		.expr(advisory_lock(
			"pg_advisory_xact_lock",
			format!("atomic:admit:{}", manifest.id),
		))
		.build(PostgresQueryBuilder);
	TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
	if let Some(existing) = AtomicCoordinator::objects()
		.filter(AtomicCoordinator::field_id().eq(manifest.id))
		.limit(1)
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?
		.pop()
	{
		if existing.digest != manifest.digest()? || existing.manifest.0 != json!(manifest) {
			return Err(Error::Conflict(
				"transaction ID already has another immutable manifest".into(),
			));
		}
		return Ok(contract(existing));
	}
	let state = AtomicCoordinator::build()
		.id(manifest.id)
		.digest(manifest.digest()?)
		.manifest(json!(manifest).into())
		.decision(None)
		.visible(false)
		.complete(false)
		.last_error(None)
		.finish();
	let saved = AtomicCoordinator::objects()
		.insert_with_executor(tx, &state)
		.await
		.map_err(FrameworkError::from)?;
	for node in &manifest.participants {
		let vote = AtomicVote::build()
			.transaction_id(manifest.id)
			.node_id(&node.node_id)
			.phase(AtomicVotePhase::Pending)
			.finish();
		AtomicVote::objects()
			.insert_with_executor(tx, &vote)
			.await
			.map_err(FrameworkError::from)?;
	}
	AtomicHistory::append(
		tx,
		manifest.id,
		"coordinator",
		"PENDING",
		"immutable manifest admitted",
	)
	.await?;
	Ok(contract(saved))
}

pub(crate) async fn touch(connection: DatabaseConnection, id: Uuid) -> Result<()> {
	connection
		.atomic(async |tx| {
			let mut state = locked(tx, id).await?;
			state.updated_at = Utc::now();
			AtomicCoordinator::objects()
				.update_with_conn(tx, &state)
				.await?;
			Ok(())
		})
		.await
}

pub(crate) async fn record_error(
	connection: DatabaseConnection,
	id: Uuid,
	error: &str,
) -> Result<()> {
	connection
		.atomic(async |tx| {
			let mut state = locked(tx, id).await?;
			state.last_error = Some(error.to_owned());
			AtomicCoordinator::objects()
				.update_with_conn(tx, &state)
				.await?;
			Ok(())
		})
		.await
}

/// Keep the vote and clearing of its transient error in one transaction.
pub(crate) async fn acknowledge(
	connection: DatabaseConnection,
	id: Uuid,
	node: &str,
	phase: AtomicVotePhase,
) -> Result<()> {
	connection
		.atomic(async |tx| {
			let mut state = locked(tx, id).await?;
			let mut vote = AtomicVote::objects()
				.filter(AtomicVote::field_transaction_id().eq(id))
				.filter(AtomicVote::field_node_id().eq(node.to_owned()))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop()
				.ok_or_else(|| Error::NotFound("transaction participant".into()))?;
			vote.phase = phase;
			// Change only the phase and require exactly the locked participant.
			let updated = AtomicVote::objects()
				.filter(AtomicVote::field_transaction_id().eq(id))
				.filter(AtomicVote::field_node_id().eq(node))
				.update_fields_with_conn(tx, [(AtomicVote::field_phase(), vote.phase)])
				.await?;
			if updated != 1 {
				return Err(Error::Conflict("transaction participant changed".into()));
			}
			state.last_error = None;
			AtomicCoordinator::objects()
				.update_with_conn(tx, &state)
				.await?;
			Ok(())
		})
		.await
}

pub(crate) async fn recovery_candidates<E: OrmExecutor>(
	db: &mut E,
	aborted: bool,
) -> Result<Vec<Uuid>> {
	let query = AtomicCoordinator::objects().filter(AtomicCoordinator::field_complete().eq(false));
	let query = if aborted {
		query.filter(AtomicCoordinator::field_decision().eq(Some(AtomicCoordinatorDecision::Abort)))
	} else {
		query.filter(
			AtomicCoordinator::field_decision()
				.is_null()
				.or(AtomicCoordinator::field_decision().ne(Some(AtomicCoordinatorDecision::Abort))),
		)
	};
	Ok(query
		.order_by(&["updated_at", "id"])
		.limit(32)
		.all_with_db(db)
		.await?
		.into_iter()
		.map(|row| row.id)
		.collect())
}

pub(super) fn advisory_lock(function: &str, key: String) -> SimpleExpr {
	let key = SimpleExpr::FunctionCall(
		Alias::new("hashtextextended").into_iden(),
		vec![Expr::value(key).into(), Expr::value(0_i64).into()],
	);
	SimpleExpr::FunctionCall(Alias::new(function).into_iden(), vec![key])
}

pub(crate) async fn recovery_lease(
	db: &BackendConnection,
	id: Uuid,
) -> Result<Box<dyn TransactionExecutor>> {
	let mut tx = db.begin().await?;
	let (sql, values) = Query::select()
		.expr_as(
			advisory_lock("pg_try_advisory_xact_lock", format!("atomic:{id}")),
			Alias::new("acquired"),
		)
		.build(PostgresQueryBuilder);
	let acquired = tx
		.fetch_one(&sql, convert_values(values))
		.await?
		.get::<bool>("acquired")
		.map_err(FrameworkError::from)?;
	if !acquired {
		tx.rollback().await?;
		return Err(Error::TransactionPending);
	}
	Ok(tx)
}

async fn locked<E: OrmExecutor + TransactionExecutor>(
	db: &mut E,
	id: Uuid,
) -> Result<AtomicCoordinator> {
	AtomicCoordinator::objects()
		.filter(AtomicCoordinator::field_id().eq(id))
		.select_for_update()
		.all_with_executor(db)
		.await
		.map_err(FrameworkError::from)?
		.pop()
		.ok_or_else(|| Error::NotFound("transaction".into()))
}

pub(crate) async fn audit<E: OrmExecutor>(db: &mut E, id: Uuid) -> Result<Vec<TransactionHistory>> {
	Ok(AtomicHistory::objects()
		.filter(AtomicHistory::field_transaction_id().eq(id))
		.order_by(&["sequence"])
		.all_with_db(db)
		.await?
		.into_iter()
		.map(|row| TransactionHistory {
			sequence: row.sequence,
			transaction_id: row.transaction_id,
			role: match row.role {
				AtomicHistoryRole::Coordinator => "coordinator",
				AtomicHistoryRole::Participant => "participant",
				AtomicHistoryRole::Trust => "trust",
			}
			.to_owned(),
			phase: row.phase,
			detail: row.detail,
			created_at: row.created_at,
		})
		.collect())
}

pub(crate) async fn status_in(tx: &mut dyn TransactionExecutor, id: Uuid) -> Result<Status> {
	AtomicCoordinator::objects()
		.filter(AtomicCoordinator::field_id().eq(id))
		.limit(1)
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?
		.pop()
		.map(contract)
		.ok_or_else(|| Error::NotFound("transaction".into()))
}

/// Stable keyset pages preserve capacity after live authority filtering.
pub(crate) async fn visible_candidates<E: OrmExecutor>(
	db: &mut E,
	identity: Option<&aidash_domain::identity::execution::ExecutionPrincipal>,
	cursor: Option<(chrono::DateTime<Utc>, Uuid)>,
) -> Result<Vec<Status>> {
	use reinhardt::query::{Cond, ExprTrait, Order};
	let mut query = Query::select();
	query
		.expr(Expr::asterisk())
		.from_as(Alias::new("atomic_coordinators"), Alias::new("c"))
		.order_by((Alias::new("c"), Alias::new("created_at")), Order::Desc)
		.order_by((Alias::new("c"), Alias::new("id")), Order::Desc)
		.limit(200);
	if let Some(identity) = identity {
		query.and_where(Expr::exists(
			Query::select()
				.expr(Expr::value(1_i64))
				.from_as(Alias::new("atomic_subjects"), Alias::new("s"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col((
						Alias::new("s"),
						Alias::new("id"),
					)))
					.eq(Expr::col((Alias::new("c"), Alias::new("id")))),
				)
				.and_where(Expr::cust("s.binding->>'tenant'").eq(Expr::value(&identity.tenant)))
				.and_where(Expr::cust("s.binding->>'subject'").eq(Expr::value(&identity.subject)))
				.to_owned(),
		));
	}
	if let Some((created, id)) = cursor {
		query.and_where(
			Cond::any()
				.add(
					Expr::col((Alias::new("c"), Alias::new("created_at"))).lt(Expr::value(created)),
				)
				.add(
					Cond::all()
						.add(
							Expr::col((Alias::new("c"), Alias::new("created_at")))
								.eq(Expr::value(created)),
						)
						.add(Expr::col((Alias::new("c"), Alias::new("id"))).lt(Expr::value(id))),
				),
		);
	}
	let (sql, values) = query.build(PostgresQueryBuilder);
	db.fetch_all(&sql, convert_values(values))
		.await?
		.into_iter()
		.map(|row| {
			serde_json::from_value(QueryRow::from_backend_row(row).data).map_err(Error::from)
		})
		.collect()
}
