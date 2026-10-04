//! Native obligation mutations retain triggers, publication epochs and lease fences.
use super::Result;
use crate::{Error, apps::execution::activation::models::RunActivations, store::Store};
use aidash_domain::{Run, activation::Obligation};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{QueryRow, execution::convert_values};
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait, Func, IntoIden, LockBehavior, LockType, OnConflict, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

fn a(name: &str) -> Alias {
	Alias::new(name)
}

pub(super) fn obligation(data: serde_json::Value) -> Result<Obligation> {
	let record: RunActivations = serde_json::from_value(data)?;
	Ok(Obligation {
		id: record.id,
		generation: record.generation,
		run_id: record.run_id,
		run_revision: record.run_revision,
		state: record.state,
		publication_epoch: record.publication_epoch,
	})
}

fn request_statement(id: Uuid, run_id: Uuid) -> reinhardt::query::InsertStatement {
	let select = Query::select()
		.expr(Expr::value(id))
		.column(a("id"))
		.column(a("revision"))
		.expr(Expr::value("handoff"))
		.from(a("runs"))
		.and_where(Expr::col("id").eq(Expr::value(run_id)))
		.to_owned();
	Query::insert()
		.into_table(a("run_activations"))
		.columns([a("id"), a("run_id"), a("run_revision"), a("reason")])
		.from_subquery(select)
		.to_owned()
}

/// Borrow the existing compatibility writer's transaction without opening another one.
pub async fn request_in(tx: &mut Transaction<'_, Postgres>, run_id: Uuid) -> Result<Uuid> {
	let id = Uuid::new_v4();
	if sqlx::query(&request_statement(id, run_id).to_string(PostgresQueryBuilder))
		.execute(&mut **tx)
		.await?
		.rows_affected()
		!= 1
	{
		return Err(Error::NotFound("activation Run".into()));
	}
	Ok(id)
}

async fn request_native(tx: &mut dyn TransactionExecutor, run_id: Uuid) -> Result<Uuid> {
	let id = Uuid::new_v4();
	let (sql, values) = request_statement(id, run_id).build(PostgresQueryBuilder);
	if tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		!= 1
	{
		return Err(Error::NotFound("activation Run".into()));
	}
	Ok(id)
}

pub(super) async fn record_claim(
	tx: &mut dyn TransactionExecutor,
	id: Uuid,
	run: &Run,
	token: Uuid,
	seconds: i32,
	source: &str,
) -> Result<()> {
	let mut query = Query::update();
	query
		.table(a("run_activations"))
		.value(a("state"), "claimed")
		.value(a("lease_token"), token)
		.value_expr(a("claimed_at"), Expr::current_timestamp())
		.value_expr(a("due_at"), crate::database::lease_deadline(seconds))
		.value(a("claim_source"), source)
		.value(a("worker_pid"), i64::from(std::process::id()))
		.value(a("disposition"), "lease_committed")
		.and_where(Expr::col("id").eq(Expr::value(id)));
	// Recovery inserts the current revision; notifications replace the earlier one.
	if source == "notification" {
		query.value(a("run_revision"), run.revision);
	}
	let (sql, values) = query.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

pub(super) async fn quarantine(
	store: &Store,
	payload: &[u8],
	reason: &'static str,
	sequence: Option<u64>,
) -> Result<()> {
	let digest = format!("{:x}", Sha256::digest(payload));
	let (sql, values) = Query::insert()
		.into_table(a("activation_quarantine"))
		.columns([a("digest"), a("reason"), a("stream_sequence")])
		.from_subquery(
			Query::select()
				.expr(Expr::value(digest))
				.expr(Expr::value(reason))
				.expr(Expr::value(sequence.and_then(|n| i64::try_from(n).ok())))
				.to_owned(),
		)
		.on_conflict(OnConflict::column(a("digest")).do_nothing().to_owned())
		.build(PostgresQueryBuilder);
	store
		.database()
		.execute(&sql, convert_values(values))
		.await?;
	metrics::counter!("aidash_activation_quarantined_total", "reason" => reason).increment(1);
	Ok(())
}

pub(super) async fn reconcile(store: &Store) -> Result<u64> {
	let existing = Query::select()
		.expr(Expr::value(1_i64))
		.from_as(a("run_activations"), a("a"))
		.and_where(Expr::col((a("a"), a("run_id"))).equals((a("runs"), a("id"))))
		.to_owned();
	let select = Query::select()
		.column(a("id"))
		.column(a("revision"))
		.expr(Expr::value("reconcile"))
		.from(a("runs"))
		.and_where(Expr::col("phase").is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
		.and_where(Expr::exists(existing).not())
		.order_by(a("updated_at"), Order::Asc)
		.limit(aidash_application::activation::RECONCILE_BATCH_SIZE)
		.to_owned();
	let (sql, values) = Query::insert()
		.into_table(a("run_activations"))
		.columns([a("run_id"), a("run_revision"), a("reason")])
		.from_subquery(select)
		.build(PostgresQueryBuilder);
	Ok(store
		.database()
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected)
}

pub(super) async fn publish_batch(store: &Store, token: Uuid) -> Result<Vec<Obligation>> {
	let due = |state: Option<&str>| {
		let mut query = Query::select();
		query
			.column(a("generation"))
			.from(a("run_activations"))
			.and_where(Expr::col("state").ne(Expr::value("settled")))
			.and_where(Expr::col("due_at").lte(Expr::current_timestamp()))
			.cond_where(
				Condition::any()
					.add(Expr::col("publish_until").is_null())
					.add(Expr::col("publish_until").lte(Expr::current_timestamp())),
			)
			.order_by(a("due_at"), Order::Asc)
			.order_by(a("generation"), Order::Asc)
			.limit(64)
			.lock(LockType::Update)
			.lock_behavior(LockBehavior::SkipLocked);
		if let Some(state) = state {
			query.and_where(Expr::col("state").eq(Expr::value(state)));
		} else {
			query.and_where(Expr::col("state").is_in(["pending", "deferred"]));
		}
		query.to_owned()
	};
	// Correlated owner checks preserve heartbeat postponement and supersession.
	let owner = Query::select()
		.expr(Expr::value(1_i64))
		.from_as(a("runs"), a("r"))
		.and_where(Expr::col((a("r"), a("id"))).equals((a("run_activations"), a("run_id"))))
		.and_where(
			Expr::col((a("r"), a("lease_owner"))).equals((a("run_activations"), a("lease_token"))),
		)
		.to_owned();
	let mut live = owner.clone();
	live.and_where(Expr::col((a("r"), a("lease_until"))).gt(Expr::current_timestamp()));
	let state = Expr::case()
		.when(
			Expr::exists(owner),
			Expr::case()
				.when(Expr::exists(live), Expr::value("claimed"))
				.else_result(Expr::value("pending")),
		)
		.else_result(Expr::value("settled"));
	let deadline = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"GREATEST".into_iden(),
			vec![
				Expr::current_timestamp().into(),
				Expr::col((a("r"), a("lease_until"))).into(),
			],
		))
		.from_as(a("runs"), a("r"))
		.and_where(Expr::col((a("r"), a("id"))).equals((a("run_activations"), a("run_id"))))
		.and_where(
			Expr::col((a("r"), a("lease_owner"))).equals((a("run_activations"), a("lease_token"))),
		)
		.to_owned();
	let (sql, values) = Query::update()
		.table(a("run_activations"))
		.value_expr(a("state"), state)
		.value_expr(a("due_at"), SimpleExpr::SubQuery(None, Box::new(deadline)))
		.value_expr(
			a("publication_epoch"),
			Expr::col("publication_epoch").add(Expr::value(1_i64)),
		)
		.value_expr(
			a("published_at"),
			Expr::value(None::<chrono::DateTime<chrono::Utc>>),
		)
		.and_where(Expr::col("generation").in_subquery(due(Some("claimed"))))
		.build(PostgresQueryBuilder);
	store
		.database()
		.execute(&sql, convert_values(values))
		.await?;
	let (sql, values) = Query::update()
		.table(a("run_activations"))
		.value(a("state"), "pending")
		.value_expr(
			a("publication_epoch"),
			Expr::col("publication_epoch").add(
				Expr::case()
					.when(Expr::col("published_at").is_null(), Expr::value(0_i64))
					.else_result(Expr::value(1_i64)),
			),
		)
		.value_expr(
			a("published_at"),
			Expr::value(None::<chrono::DateTime<chrono::Utc>>),
		)
		.value(a("publish_token"), token)
		.value_expr(a("publish_until"), crate::database::lease_deadline(5))
		.and_where(Expr::col("generation").in_subquery(due(None)))
		.returning_all()
		.build(PostgresQueryBuilder);
	store
		.database()
		.fetch_all(&sql, convert_values(values))
		.await?
		.into_iter()
		.map(|row| obligation(QueryRow::from_backend_row(row).data))
		.collect()
}

pub(super) async fn published(store: &Store, row: &Obligation, token: Uuid) -> Result<()> {
	let (sql, values) = Query::update()
		.table(a("run_activations"))
		.value_expr(a("published_at"), Expr::current_timestamp())
		.value_expr(a("due_at"), crate::database::lease_deadline(5))
		.value_expr(
			a("publish_until"),
			Expr::value(None::<chrono::DateTime<chrono::Utc>>),
		)
		.and_where(Expr::col("id").eq(Expr::value(row.id)))
		.and_where(Expr::col("state").eq(Expr::value("pending")))
		.and_where(Expr::col("publication_epoch").eq(Expr::value(row.publication_epoch)))
		.and_where(Expr::col("publish_token").eq(Expr::value(token)))
		.build(PostgresQueryBuilder);
	store
		.database()
		.execute(&sql, convert_values(values))
		.await?;
	Ok(())
}

pub(super) async fn recover(store: &Store, seconds: i32) -> Result<Option<(Run, Uuid)>> {
	let mut tx = store.database().begin().await?;
	let token = Uuid::new_v4();
	let mut cursor = store.recovery_cursors.execution.lock().await;
	let Some(run) = Store::lease_run_in(
		tx.as_mut(),
		token,
		seconds,
		None,
		&store.node_id,
		&mut cursor,
	)
	.await?
	else {
		tx.commit().await?;
		return Ok(None);
	};
	let id = request_native(tx.as_mut(), run.id).await?;
	record_claim(tx.as_mut(), id, &run, token, seconds, "recovery").await?;
	tx.commit().await?;
	metrics::counter!("aidash_activation_claims_total", "source" => "recovery").increment(1);
	tracing::info!(run_id=%run.id, activation_id=%id, worker_pid=std::process::id(), "recovery lease committed");
	Ok(Some((run, token)))
}

pub(super) async fn observe(store: &Store) -> Result<()> {
	let (sql, values) = Query::select()
		.expr_as(
			Func::count(Expr::col(reinhardt::query::ColumnRef::Asterisk).into()),
			a("pending"),
		)
		.expr_as(
			Func::coalesce(vec![
				SimpleExpr::FunctionCall(
					"date_part".into_iden(),
					vec![
						Expr::value("epoch").into(),
						Expr::current_timestamp()
							.sub(Func::min(Expr::col("due_at").into()))
							.into(),
					],
				),
				Expr::value(0_f64).into(),
			]),
			a("age"),
		)
		.from(a("run_activations"))
		.and_where(Expr::col("state").ne(Expr::value("settled")))
		.and_where(Expr::col("due_at").lte(Expr::current_timestamp()))
		.build(PostgresQueryBuilder);
	let row = store
		.database()
		.fetch_one(&sql, convert_values(values))
		.await?;
	let pending: i64 = row.get("pending").map_err(FrameworkError::from)?;
	let age: f64 = row.get("age").map_err(FrameworkError::from)?;
	metrics::gauge!("aidash_activation_due_obligations").set(pending as f64);
	metrics::gauge!("aidash_activation_oldest_due_seconds").set(age.max(0.));
	Ok(())
}
