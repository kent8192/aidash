use super::{LocalStatus, Manifest, Status, Vote, history, participant};
use crate::{Error, Result, federation::Federation};
use chrono::Utc;
use reqwest::Method;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

pub async fn status(f: &Federation, id: Uuid) -> Result<Status> {
	sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new("atomic_coordinators"))
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_optional(&f.store.control_pool)
	.await?
	.ok_or_else(|| Error::NotFound("transaction".into()))
}
pub async fn votes(f: &Federation, id: Uuid) -> Result<Vec<Vote>> {
	Ok(sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("node_id")),
			))
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("phase")),
			))
			.from(sea_orm::sea_query::Alias::new("atomic_votes"))
			.and_where(sea_orm::sea_query::Expr::cust("transaction_id = $1"))
			.order_by_expr(
				sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
					sea_orm::sea_query::Alias::new("node_id"),
				)),
				sea_orm::sea_query::Order::Asc,
			)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_all(&f.store.control_pool)
	.await?)
}
pub async fn submit(f: &Federation, manifest: &Manifest) -> Result<Status> {
	manifest.validate()?;
	if manifest.coordinator != f.config.node_id {
		return Err(Error::Invalid("submit to the named coordinator".into()));
	}
	match status(f, manifest.id).await {
		Ok(existing) => {
			if existing.digest != manifest.digest()? || existing.manifest != json!(manifest) {
				return Err(Error::Conflict(
					"transaction ID already has another immutable manifest".into(),
				));
			}
			return Ok(existing);
		}
		Err(Error::NotFound(_)) => {}
		Err(error) => return Err(error),
	}
	let remaining = manifest
		.deadline
		.signed_duration_since(Utc::now())
		.num_seconds();
	if !(1..=3600).contains(&remaining) {
		return Err(Error::Invalid(
			"new transaction deadline must be within the next hour".into(),
		));
	}
	for node in &manifest.participants {
		if node.node_id != f.config.node_id {
			f.peer(&node.node_id).await?;
			let allowed: bool = sqlx::query_scalar(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::Expr::cust(
						"EXISTS(SELECT 1 FROM atomic_peer_trust WHERE node_id = $1 AND enabled)",
					))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(&node.node_id)
			.fetch_one(&f.store.control_pool)
			.await?;
			if !allowed {
				return Err(Error::Forbidden);
			}
		}
	}
	let mut tx = f.store.control_pool.begin().await?;
	let inserted = sqlx::query(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("atomic_coordinators"))
			.columns([
				sea_orm::sea_query::Alias::new("id"),
				sea_orm::sea_query::Alias::new("digest"),
				sea_orm::sea_query::Alias::new("manifest"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("$2"),
				sea_orm::sea_query::Expr::cust("$3"),
			])
			.on_conflict(
				sea_orm::sea_query::OnConflict::new()
					.do_nothing()
					.to_owned(),
			)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(manifest.id)
	.bind(manifest.digest()?)
	.bind(json!(manifest))
	.execute(&mut *tx)
	.await?
	.rows_affected();
	let stored: Status = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new("atomic_coordinators"))
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(manifest.id)
	.fetch_one(&mut *tx)
	.await?;
	if stored.digest != manifest.digest()? || stored.manifest != json!(manifest) {
		return Err(Error::Conflict(
			"transaction ID already has another immutable manifest".into(),
		));
	}
	if inserted == 1 {
		for node in &manifest.participants {
			sqlx::query(
				&sea_orm::sea_query::Query::insert()
					.into_table(sea_orm::sea_query::Alias::new("atomic_votes"))
					.columns([
						sea_orm::sea_query::Alias::new("transaction_id"),
						sea_orm::sea_query::Alias::new("node_id"),
					])
					.values_panic([
						sea_orm::sea_query::Expr::cust("$1"),
						sea_orm::sea_query::Expr::cust("$2"),
					])
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(manifest.id)
			.bind(&node.node_id)
			.execute(&mut *tx)
			.await?;
		}
		history(
			&mut tx,
			manifest.id,
			"coordinator",
			"PENDING",
			"immutable manifest admitted",
		)
		.await?;
	}
	tx.commit().await?;
	f.notify.notify_waiters();
	Ok(stored)
}
pub(crate) async fn remote<T: DeserializeOwned>(
	f: &Federation,
	node: &str,
	method: Method,
	path: &str,
	body: Option<&impl Serialize>,
) -> Result<T> {
	let value = body.map(serde_json::to_value).transpose()?;
	tokio::time::timeout(Duration::from_secs(10), async {
		let response = f.peer_response(node, method, path, value.as_ref()).await?;
		let status = response.status();
		if !status.is_success() {
			return Err(match status {
				reqwest::StatusCode::BAD_REQUEST
				| reqwest::StatusCode::UNPROCESSABLE_ENTITY
				| reqwest::StatusCode::NOT_FOUND
				| reqwest::StatusCode::METHOD_NOT_ALLOWED => Error::Invalid(format!(
					"transaction participant rejected request: {status}"
				)),
				reqwest::StatusCode::UNAUTHORIZED => Error::Unauthorized,
				reqwest::StatusCode::FORBIDDEN => Error::Forbidden,
				reqwest::StatusCode::CONFLICT => Error::Conflict(
					"transaction participant rejected its state precondition".into(),
				),
				reqwest::StatusCode::SERVICE_UNAVAILABLE => Error::TransactionPending,
				_ => Error::External(format!("transaction participant returned {status}")),
			});
		}
		crate::response::json(response, 4_194_304).await
	})
	.await
	.map_err(|_| {
		Error::External(
			"transaction participant response timed out; outcome retained for recovery".into(),
		)
	})?
}
pub(crate) async fn decision(f: &Federation, manifest: &Manifest) -> Result<Status> {
	let proof: Status = if manifest.coordinator == f.config.node_id {
		status(f, manifest.id).await?
	} else {
		remote(
			f,
			&manifest.coordinator,
			Method::GET,
			&format!("/transactions/{}/decision", manifest.id),
			None::<&()>,
		)
		.await?
	};
	if proof.id != manifest.id
		|| proof.digest != manifest.digest()?
		|| proof.manifest != json!(manifest)
		|| proof
			.decision
			.as_deref()
			.is_some_and(|d| !matches!(d, "COMMIT" | "ABORT"))
		|| (proof.visible && proof.decision.as_deref() != Some("COMMIT"))
	{
		return Err(Error::Conflict(
			"coordinator decision does not match participant manifest".into(),
		));
	}
	Ok(proof)
}
async fn send(f: &Federation, manifest: &Manifest, node: &str, phase: &str) -> Result<LocalStatus> {
	if node == f.config.node_id {
		match phase {
			"reserve" => participant::reserve(f, &manifest.coordinator, manifest).await,
			"prepare" => participant::prepare(f, &manifest.coordinator, manifest).await,
			_ => participant::finish(f, &manifest.coordinator, manifest).await,
		}
	} else {
		remote(
			f,
			node,
			Method::POST,
			&format!("/transactions/{phase}"),
			Some(manifest),
		)
		.await
	}
}
async fn record_decision(f: &Federation, id: Uuid, decision: &str, reason: &str) -> Result<()> {
	let mut tx = f.store.control_pool.begin().await?;
	let changed = sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("atomic_coordinators"))
			.value(
				sea_orm::sea_query::Alias::new("decision"),
				sea_orm::sea_query::Expr::cust("$2"),
			)
			.value(
				sea_orm::sea_query::Alias::new("last_error"),
				sea_orm::sea_query::Expr::cust("$3"),
			)
			.and_where(sea_orm::sea_query::Expr::cust(
				"id = $1 AND decision IS NULL",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(id)
	.bind(decision)
	.bind((!reason.is_empty()).then_some(reason))
	.execute(&mut *tx)
	.await?
	.rows_affected();
	if changed == 1 {
		history(&mut tx, id, "coordinator", decision, reason).await?;
	}
	tx.commit().await?;
	Ok(())
}
async fn lease(f: &Federation, id: Uuid) -> Result<sqlx::Transaction<'static, sqlx::Postgres>> {
	let mut tx = f.store.control_pool.begin().await?;
	let acquired: bool = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust(
				"PG_TRY_ADVISORY_XACT_LOCK(HASHTEXTEXTENDED('atomic:' || $1, 0))",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(id.to_string())
	.fetch_one(&mut *tx)
	.await?;
	if !acquired {
		return Err(Error::TransactionPending);
	}
	Ok(tx)
}
pub async fn abort(f: &Federation, id: Uuid) -> Result<Status> {
	// The conditional decision update arbitrates commit versus abort in SQL.
	// Do not require the recovery lease: it spans peer I/O, and contention must
	// not discard an operator's abort. An in-flight transition cannot overwrite
	// this immutable decision; subsequent recovery finalizes the chosen result.
	record_decision(f, id, "ABORT", "operator requested abort").await?;
	let existing = status(f, id).await?;
	if existing.decision.as_deref() == Some("COMMIT") {
		return Err(Error::Conflict("commit is irrevocable".into()));
	}
	Ok(existing)
}

/// Make one durable protocol transition. Recovery repeats this exact function;
/// it needs no process-local state or ownership inferred from a timeout.
pub async fn advance(f: &Federation, id: Uuid) -> Result<Status> {
	let lease = lease(f, id).await?;
	let result = advance_locked(f, id).await;
	// Await advisory-lock release rather than relying on SQLx's asynchronous
	// rollback-on-drop before acknowledging the transition to its caller.
	lease.commit().await?;
	result
}
async fn advance_locked(f: &Federation, id: Uuid) -> Result<Status> {
	let state = status(f, id).await?;
	if state.complete {
		return Ok(state);
	}
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("atomic_coordinators"))
			.value(
				sea_orm::sea_query::Alias::new("updated_at"),
				sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(id)
	.execute(&f.store.control_pool)
	.await?;
	let manifest: Manifest = serde_json::from_value(state.manifest.clone())?;
	if state.decision.is_none() && manifest.deadline <= Utc::now() {
		record_decision(f, id, "ABORT", "deadline elapsed before durable decision").await?;
		return status(f, id).await;
	}
	let votes = votes(f, id).await?;
	let selected = if state.decision.is_none() {
		votes
			.iter()
			.find(|v| v.phase == "PENDING")
			.map(|v| (v, "reserve"))
			.or_else(|| {
				votes
					.iter()
					.find(|v| v.phase == "RESERVED")
					.map(|v| (v, "prepare"))
			})
	} else if state.decision.as_deref() == Some("ABORT") {
		votes
			.iter()
			.find(|v| v.phase != "ABORTED")
			.map(|v| (v, "finish"))
	} else if !state.visible {
		votes
			.iter()
			.find(|v| !matches!(v.phase.as_str(), "APPLIED" | "COMMITTED"))
			.map(|v| (v, "finish"))
	} else {
		votes
			.iter()
			.find(|v| v.phase != "COMMITTED")
			.map(|v| (v, "finish"))
	};
	if let Some((vote, operation)) = selected {
		match send(f, &manifest, &vote.node_id, operation).await {
			Ok(result) => {
				if result.id != manifest.id
					|| result.coordinator != manifest.coordinator
					|| result.digest != state.digest
					|| result.manifest != state.manifest
				{
					return Err(Error::Conflict(
						"participant acknowledged another manifest".into(),
					));
				}
				let expected = match operation {
					"reserve" => "RESERVED",
					"prepare" => "PREPARED",
					_ if state.decision.as_deref() == Some("ABORT") => "ABORTED",
					_ if state.visible => "COMMITTED",
					_ => "APPLIED",
				};
				if result.phase != expected {
					return Err(Error::Conflict(
						"participant returned an unexpected phase".into(),
					));
				}
				sqlx::query(
					&sea_orm::sea_query::Query::update()
						.table(sea_orm::sea_query::Alias::new("atomic_votes"))
						.value(
							sea_orm::sea_query::Alias::new("phase"),
							sea_orm::sea_query::Expr::cust("$3"),
						)
						.and_where(sea_orm::sea_query::Expr::cust(
							"transaction_id = $1 AND node_id = $2",
						))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(id)
				.bind(&vote.node_id)
				.bind(&result.phase)
				.execute(&f.store.control_pool)
				.await?;
				sqlx::query(
					&sea_orm::sea_query::Query::update()
						.table(sea_orm::sea_query::Alias::new("atomic_coordinators"))
						.value(
							sea_orm::sea_query::Alias::new("last_error"),
							sea_orm::sea_query::Expr::cust("NULL"),
						)
						.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(id)
				.execute(&f.store.control_pool)
				.await?;
			}
			Err(error) => {
				if state.decision.is_none()
					&& matches!(
						error,
						Error::Conflict(_)
							| Error::Invalid(_) | Error::NotFound(_)
							| Error::Forbidden | Error::Unauthorized
					) {
					record_decision(f, id, "ABORT", &error.to_string()).await?;
				} else {
					sqlx::query(
						&sea_orm::sea_query::Query::update()
							.table(sea_orm::sea_query::Alias::new("atomic_coordinators"))
							.value(
								sea_orm::sea_query::Alias::new("last_error"),
								sea_orm::sea_query::Expr::cust("$2"),
							)
							.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
							.to_string(sea_orm::sea_query::PostgresQueryBuilder),
					)
					.bind(id)
					.bind(error.to_string())
					.execute(&f.store.control_pool)
					.await?;
				}
			}
		}
	} else if state.decision.is_none() {
		if !votes.iter().all(|v| v.phase == "PREPARED") {
			return Err(Error::Conflict(
				"commit requires every prepared vote".into(),
			));
		}
		record_decision(f, id, "COMMIT", "").await?;
	} else {
		let mut tx = f.store.control_pool.begin().await?;
		if state.decision.as_deref() == Some("COMMIT") && !state.visible {
			sqlx::query(
				&sea_orm::sea_query::Query::update()
					.table(sea_orm::sea_query::Alias::new("atomic_coordinators"))
					.value(
						sea_orm::sea_query::Alias::new("visible"),
						sea_orm::sea_query::Expr::cust("TRUE"),
					)
					.value(
						sea_orm::sea_query::Alias::new("last_error"),
						sea_orm::sea_query::Expr::cust("NULL"),
					)
					.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(id)
			.execute(&mut *tx)
			.await?;
			history(
				&mut tx,
				id,
				"coordinator",
				"VISIBLE",
				"every participant durably applied commit",
			)
			.await?;
		} else {
			sqlx::query(
				&sea_orm::sea_query::Query::update()
					.table(sea_orm::sea_query::Alias::new("atomic_coordinators"))
					.value(
						sea_orm::sea_query::Alias::new("complete"),
						sea_orm::sea_query::Expr::cust("TRUE"),
					)
					.value(
						sea_orm::sea_query::Alias::new("last_error"),
						sea_orm::sea_query::Expr::cust("NULL"),
					)
					.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(id)
			.execute(&mut *tx)
			.await?;
			history(
				&mut tx,
				id,
				"coordinator",
				"COMPLETE",
				"every participant finalized",
			)
			.await?;
		}
		tx.commit().await?;
	}
	status(f, id).await
}
pub async fn recover_once(f: &Federation) -> Result<()> {
	let ids: Vec<Uuid> = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("id")),
			))
			.from(sea_orm::sea_query::Alias::new("atomic_coordinators"))
			.and_where(sea_orm::sea_query::Expr::cust("NOT complete"))
			.order_by_expr(
				sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
					sea_orm::sea_query::Alias::new("updated_at"),
				)),
				sea_orm::sea_query::Order::Asc,
			)
			.order_by_expr(
				sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
					sea_orm::sea_query::Alias::new("id"),
				)),
				sea_orm::sea_query::Order::Asc,
			)
			.limit(32)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_all(&f.store.control_pool)
	.await?;
	for id in ids {
		if let Err(error) = advance(f, id).await {
			tracing::warn!(%id,%error,"atomic transaction recovery pending");
		}
	}
	Ok(())
}
pub async fn run(f: Federation) -> Result<()> {
	loop {
		if let Err(error) = recover_once(&f).await {
			tracing::warn!(%error,"coordinator recovery failed; decisions retained");
		}
		tokio::time::sleep(Duration::from_millis(100)).await;
	}
}
