//! Durable dispatch fencing. No transaction is kept open during backoff. A
//! dispatched attempt is never reissued, including after a process crash.
use super::{Binding, Failure, Operation, Receipt};
use crate::{Error, Result, store::Store};
use chrono::{DateTime, Utc};
use reinhardt::query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use uuid::Uuid;

fn a(name: &str) -> Alias {
	Alias::new(name)
}
fn sql(q: reinhardt::query::SelectStatement) -> String {
	q.to_string(PostgresQueryBuilder)
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct Record {
	pub id: Uuid,
	pub home_node: String,
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub digest: String,
	pub binding: Value,
	pub state: String,
	pub cycle: i32,
	pub failures: i32,
	pub attempt_id: Option<Uuid>,
	pub fence: i64,
	pub lease_until: Option<DateTime<Utc>>,
	pub next_attempt: Option<DateTime<Utc>>,
	pub error: Option<String>,
	pub receipt: Option<Value>,
}
#[derive(Debug, Clone)]
pub(crate) struct Attempt {
	pub operation_id: Uuid,
	pub id: Uuid,
	pub fence: i64,
}
pub(crate) enum Claim {
	Attempt(Attempt),
	Ready(Box<Receipt>),
}

fn reason(value: Option<&str>) -> Failure {
	value
		.and_then(|s| serde_json::from_value(json!(s)).ok())
		.unwrap_or(Failure::Unavailable)
}
fn code(reason: Failure) -> Result<String> {
	serde_json::to_value(reason)?
		.as_str()
		.map(str::to_owned)
		.ok_or(Error::RemoteSemantic(Failure::ProviderContract))
}

/// Five automatic retries follow the initial attempt. This counter is stored
/// on the operation, not in an in-memory worker or an HTTP transport loop.
pub(crate) fn retry_delay(failures: i32) -> Option<i64> {
	(1..=5).contains(&failures).then(|| 1_i64 << failures)
}

pub(crate) async fn prepare(
	store: &Store,
	operation: &Operation,
	binding: &Binding,
) -> Result<Record> {
	operation.validate()?;
	let digest = operation.digest()?;
	let value = json!({"operation":operation,"semantic":binding});
	let mut tx = store.pool.begin().await?;
	sqlx::query(&format!(
		"{} ON CONFLICT DO NOTHING",
		Query::insert()
			.into_table(a("semantic_remote_operations"))
			.columns(
				[
					"id",
					"home_node",
					"grant_id",
					"admission_id",
					"digest",
					"binding",
				]
				.map(a),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::cust("$1"))
					.expr(Expr::cust("$2"))
					.expr(Expr::cust("$3"))
					.expr(Expr::cust("$4"))
					.expr(Expr::cust("$5"))
					.expr(Expr::cust("$6"))
					.to_owned()
			)
			.to_owned()
			.to_string(PostgresQueryBuilder)
	))
	.bind(operation.id)
	.bind(&operation.home_node)
	.bind(operation.grant_id)
	.bind(operation.admission_id)
	.bind(&digest)
	.bind(&value)
	.execute(&mut *tx)
	.await?;
	let record: Record = sqlx::query_as(&sql(Query::select()
		.column(Asterisk)
		.from(a("semantic_remote_operations"))
		.and_where(reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(Expr::cust("$1")))
		.lock(LockType::Share)
		.to_owned()))
	.bind(operation.id)
	.fetch_one(&mut *tx)
	.await?;
	if record.home_node != operation.home_node
		|| record.grant_id != operation.grant_id
		|| record.admission_id != operation.admission_id
		|| record.digest != digest
		|| record.binding != value
	{
		return Err(Error::Conflict(
			"semantic operation id already binds different inputs".into(),
		));
	}
	tx.commit().await?;
	Ok(record)
}

pub(crate) async fn bound(
	store: &Store,
	operation: &Operation,
	binding: &Binding,
) -> Result<Record> {
	let record: Record = sqlx::query_as(&sql(Query::select()
		.column(Asterisk)
		.from(a("semantic_remote_operations"))
		.and_where(reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(Expr::cust("$1")))
		.to_owned()))
	.bind(operation.id)
	.fetch_optional(&store.pool)
	.await?
	.ok_or(Error::Forbidden)?;
	if record.digest != operation.digest()?
		|| record.binding != json!({"operation":operation,"semantic":binding})
	{
		return Err(Error::Forbidden);
	}
	Ok(record)
}

pub(crate) async fn claim(store: &Store, id: Uuid) -> Result<Claim> {
	let mut tx = store.pool.begin().await?;
	let record: Record = sqlx::query_as(&sql(Query::select()
		.column(Asterisk)
		.from(a("semantic_remote_operations"))
		.and_where(reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(Expr::cust("$1")))
		.lock(LockType::Update)
		.to_owned()))
	.bind(id)
	.fetch_one(&mut *tx)
	.await?;
	let now: DateTime<Utc> = sqlx::query_scalar(&sql(Query::select()
		.expr(Expr::cust("CLOCK_TIMESTAMP()"))
		.to_owned()))
	.fetch_one(&mut *tx)
	.await?;
	match record.state.as_str() {
		"READY" => {
			let receipt = serde_json::from_value(
				record
					.receipt
					.ok_or(Error::RemoteSemantic(Failure::ProviderContract))?,
			)?;
			tx.commit().await?;
			return Ok(Claim::Ready(Box::new(receipt)));
		}
		"PAUSED" => return Err(Error::RemoteSemantic(reason(record.error.as_deref()))),
		"INVALIDATED" => return Err(Error::RemoteSemantic(Failure::Invalidated)),
		"CANCELLED" => return Err(Error::RemoteSemantic(Failure::Authority)),
		_ => {}
	}
	if record.next_attempt.is_some_and(|due| due > now)
		|| record.lease_until.is_some_and(|until| until > now)
	{
		return Err(Error::RemoteSemantic(Failure::Pending));
	}
	if record.state == "ACTIVE" {
		// Expired ownership is not proof that an HTTP request was unsent. Fence
		// the old owner and conservatively retain all dispatched reservations.
		if let Some(previous) = record.attempt_id {
			{
				let query_bind_1 = previous;
				sqlx::query(
					&Query::update()
						.table(a("semantic_remote_attempts"))
						.value_expr(
							a("state"),
							Expr::cust(
								"CASE WHEN state = 'RESERVING' THEN 'ABORTED' ELSE 'UNCERTAIN' END",
							),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=? AND state IN ('RESERVING','DISPATCHED'))".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?
			};
		}
		let failures = record.failures + 1;
		let delay = retry_delay(failures);
		{
			let query_bind_1 = id;
			let query_bind_2 = if delay.is_some() { "WAITING" } else { "PAUSED" };
			let query_bind_3 = delay.map(|s| s as f64);
			let query_bind_4 = code(if delay.is_some() {
				Failure::Unavailable
			} else {
				Failure::RetriesExhausted
			})?;
			sqlx::query(
				&Query::update()
					.table(a("semantic_remote_operations"))
					.value_expr(
						a("state"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.value(a("failures"), failures)
					.value_expr(
						a("fence"),
						Expr::col(a("fence")).add(reinhardt::query::Expr::value(1)),
					)
					.value_expr(a("lease_until"), Expr::cust("NULL"))
					.value_expr(
						a("next_attempt"),
						SimpleExpr::CustomWithExpr(
							"(CLOCK_TIMESTAMP() + MAKE_INTERVAL(secs => ?))".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.value_expr(
						a("error"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						),
					)
					.and_where(reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?
		};
		tx.commit().await?;
		return Err(Error::RemoteSemantic(if delay.is_some() {
			Failure::Pending
		} else {
			Failure::RetriesExhausted
		}));
	}
	let attempt = Attempt {
		operation_id: id,
		id: Uuid::new_v4(),
		fence: record.fence + 1,
	};
	{
		let query_bind_1 = id;
		let query_bind_2 = attempt.id;
		sqlx::query(
			&Query::update()
				.table(a("semantic_remote_operations"))
				.value(a("state"), "ACTIVE")
				.value(a("fence"), attempt.fence)
				.value_expr(
					a("attempt_id"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.value_expr(a("next_attempt"), Expr::cust("NULL"))
				.value_expr(
					a("lease_until"),
					Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '120 seconds'"),
				)
				.and_where(reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					),
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?
	};
	{
		let query_bind_1 = attempt.id;
		let query_bind_2 = id;
		sqlx::query(
			&Query::insert()
				.into_table(a("semantic_remote_attempts"))
				.columns(["id", "operation_id", "fence", "cycle", "state"].map(a))
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(Expr::value(attempt.fence))
						.expr(Expr::value(record.cycle))
						.expr(Expr::value("RESERVING"))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?
	};
	tx.commit().await?;
	Ok(Claim::Attempt(attempt))
}

async fn current(
	tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
	attempt: &Attempt,
) -> Result<Record> {
	sqlx::query_as(&sql(Query::select()
		.column(Asterisk)
		.from(a("semantic_remote_operations"))
		.and_where(Expr::cust(
			"id=$1 AND attempt_id=$2 AND fence=$3 AND state='ACTIVE' AND lease_until > CLOCK_TIMESTAMP()",
		))
		.lock(LockType::Update)
		.to_owned()))
	.bind(attempt.operation_id)
	.bind(attempt.id)
	.bind(attempt.fence)
	.fetch_optional(&mut **tx)
	.await?
	.ok_or(Error::RemoteSemantic(Failure::Pending))
}

/// Commit *before* provider I/O. Only the caller that changed RESERVING to
/// DISPATCHED may issue the request; observing DISPATCHED never grants dispatch.
pub(crate) async fn dispatched(
	store: &Store,
	attempt: &Attempt,
	reservations: &Value,
) -> Result<()> {
	let mut tx = store.pool.begin().await?;
	current(&mut tx, attempt).await?;
	let changed = {
		let query_bind_1 = attempt.id;
		let query_bind_2 = reservations;
		sqlx::query(
			&Query::update()
				.table(a("semantic_remote_attempts"))
				.value(a("state"), "DISPATCHED")
				.value_expr(
					a("reservations"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.value_expr(a("dispatched_at"), Expr::cust("CLOCK_TIMESTAMP()"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=? AND state='RESERVING')".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?
	}
	.rows_affected();
	if changed != 1 {
		return Err(Error::RemoteSemantic(Failure::Pending));
	}
	tx.commit().await?;
	Ok(())
}

pub(crate) async fn complete(store: &Store, attempt: &Attempt, receipt: &Receipt) -> Result<()> {
	let mut tx = store.pool.begin().await?;
	let record = current(&mut tx, attempt).await?;
	if record.id != receipt.operation_id
		|| record.digest != receipt.operation_digest
		|| record.home_node != receipt.home_node
		|| record.grant_id != receipt.grant_id
		|| record.admission_id != receipt.admission_id
	{
		return Err(Error::RemoteSemantic(Failure::ProviderContract));
	}
	for source in &receipt.sources {
		sqlx::query(&format!(
			"{} ON CONFLICT DO NOTHING",
			Query::insert()
				.into_table(a("semantic_remote_reads"))
				.columns(
					[
						"grant_id",
						"admission_id",
						"entry_id",
						"revision",
						"content_digest",
					]
					.map(a),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::cust("$1"))
						.expr(Expr::cust("$2"))
						.expr(Expr::cust("$3"))
						.expr(Expr::cust("$4"))
						.expr(Expr::cust("$5"))
						.to_owned()
				)
				.to_owned()
				.to_string(PostgresQueryBuilder)
		))
		.bind(receipt.grant_id)
		.bind(receipt.admission_id)
		.bind(source.entry_id)
		.bind(source.revision)
		.bind(&source.content_digest)
		.execute(&mut *tx)
		.await?;
	}
	{
		let query_bind_1 = receipt.operation_id;
		let query_bind_2 = serde_json::to_value(receipt)?;
		sqlx::query(
			&Query::update()
				.table(a("semantic_remote_operations"))
				.value(a("state"), "READY")
				.value_expr(
					a("receipt"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.value_expr(a("lease_until"), Expr::cust("NULL"))
				.value_expr(a("error"), Expr::cust("NULL"))
				.and_where(reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					),
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?
	};
	{
		let query_bind_1 = attempt.id;
		sqlx::query(
			&Query::update()
				.table(a("semantic_remote_attempts"))
				.value(a("state"), "COMPLETED")
				.value_expr(a("completed_at"), Expr::cust("CLOCK_TIMESTAMP()"))
				.and_where(reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					),
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?
	};
	tx.commit().await?;
	Ok(())
}

pub(crate) async fn failed(store: &Store, attempt: &Attempt, failure: Failure) -> Result<Failure> {
	let mut tx = store.pool.begin().await?;
	let record = current(&mut tx, attempt).await?;
	let failures = record.failures + i32::from(failure.transient());
	let delay = failure.transient().then(|| retry_delay(failures)).flatten();
	let failure = if failure.transient() && delay.is_none() {
		Failure::RetriesExhausted
	} else {
		failure
	};
	{
		let query_bind_1 = record.id;
		let query_bind_2 = code(failure)?;
		let query_bind_3 = delay.map(|s| s as f64);
		sqlx::query(
			&Query::update()
				.table(a("semantic_remote_operations"))
				.value(
					a("state"),
					if failure == Failure::Invalidated {
						"INVALIDATED"
					} else if delay.is_some() {
						"WAITING"
					} else {
						"PAUSED"
					},
				)
				.value(a("failures"), failures)
				.value_expr(
					a("error"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.value_expr(a("lease_until"), Expr::cust("NULL"))
				.value_expr(
					a("next_attempt"),
					SimpleExpr::CustomWithExpr(
						"(CLOCK_TIMESTAMP() + MAKE_INTERVAL(secs => ?))".to_owned(),
						vec![Expr::value(query_bind_3.to_owned()).into()],
					),
				)
				.and_where(reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					),
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?
	};
	{
		let query_bind_1 = attempt.id;
		let query_bind_2 = code(failure)?;
		sqlx::query(
			&Query::update()
				.table(a("semantic_remote_attempts"))
				.value_expr(
					a("state"),
					Expr::cust("CASE WHEN state='RESERVING' THEN 'ABORTED' ELSE 'UNCERTAIN' END"),
				)
				.value_expr(
					a("error"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=? AND state IN ('RESERVING','DISPATCHED'))".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?
	};
	tx.commit().await?;
	Ok(failure)
}

pub(crate) async fn expire_cached(store: &Store, id: Uuid) -> Result<()> {
	{
		let query_bind_1 = id;
		sqlx::query(
			&Query::update()
				.table(a("semantic_remote_operations"))
				.value(a("state"), "PENDING")
				.value_expr(a("receipt"), Expr::cust("NULL"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=? AND state='READY')".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&store.pool)
		.await?
	};
	Ok(())
}

/// Caller retains current grant/source authority. Re-delivery does not start
/// another cycle after PAUSED/WAITING has already become PENDING.
pub(crate) async fn resume_in(
	store: &Store,
	tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
	grant: Uuid,
	admission: Uuid,
	workspace: Uuid,
	actor: &str,
) -> Result<()> {
	let records: Vec<Record> = sqlx::query_as(&sql(Query::select()
		.column(Asterisk)
		.from(a("semantic_remote_operations"))
		.and_where(Expr::cust("grant_id=$1 AND admission_id=$2"))
		.order_by(a("id"), reinhardt::query::Order::Asc)
		.lock(LockType::Update)
		.to_owned()))
	.bind(grant)
	.bind(admission)
	.fetch_all(&mut **tx)
	.await?;
	for record in records {
		if record.state == "INVALIDATED" {
			return Err(Error::RemoteSemantic(Failure::Invalidated));
		}
		if record.state == "CANCELLED" {
			return Err(Error::RemoteSemantic(Failure::Authority));
		}
		if !matches!(record.state.as_str(), "PAUSED" | "WAITING") {
			continue;
		}
		{
			let query_bind_1 = record.id;
			sqlx::query(
				&Query::update()
					.table(a("semantic_remote_operations"))
					.value_expr(
						a("cycle"),
						Expr::col(a("cycle")).add(reinhardt::query::Expr::value(1)),
					)
					.value(a("state"), "PENDING")
					.value(a("failures"), 0)
					.value_expr(a("next_attempt"), Expr::cust("NULL"))
					.value_expr(a("lease_until"), Expr::cust("NULL"))
					.value_expr(a("error"), Expr::cust("NULL"))
					.and_where(reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await?
		};
		store
			.event(
				tx,
				Some(workspace),
				"semantic.manual_retry",
				json!({"operation_id":record.id,"grant_id":grant,"admission_id":admission,
				"cycle":record.cycle+1,"actor":actor,"reason":"authorized_resume"}),
			)
			.await?;
	}
	Ok(())
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

use reinhardt::query::ColumnRef::Asterisk;

use reinhardt::query::SimpleExpr;

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn retry_schedule_is_bounded_without_resetting_failed_attempts() {
		assert_eq!(
			(1..=6).map(retry_delay).collect::<Vec<_>>(),
			vec![Some(2), Some(4), Some(8), Some(16), Some(32), None]
		);
		assert_eq!(retry_delay(0), None);
		assert_eq!(retry_delay(i32::MAX), None);
	}
}
