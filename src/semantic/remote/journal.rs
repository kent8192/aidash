//! Durable dispatch fencing. No transaction is kept open during backoff. A
//! dispatched attempt is never reissued, including after a process crash.
use super::{Binding, Failure, Operation, Receipt};
use crate::{Error, Result, store::Store};
use chrono::{DateTime, Utc};
use sea_orm::sea_query::{
	Alias, Asterisk, Expr, LockType, OnConflict, PostgresQueryBuilder, Query,
};
use serde_json::{Value, json};
use uuid::Uuid;

fn a(name: &str) -> Alias {
	Alias::new(name)
}
fn sql(q: sea_orm::sea_query::SelectStatement) -> String {
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
	sqlx::query(
		&Query::insert()
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
			.values_panic(["$1", "$2", "$3", "$4", "$5", "$6"].map(Expr::cust))
			.on_conflict(OnConflict::new().do_nothing().to_owned())
			.to_string(PostgresQueryBuilder),
	)
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
		.and_where(Expr::col(a("id")).eq(Expr::cust("$1")))
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
		.and_where(Expr::col(a("id")).eq(Expr::cust("$1")))
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
		.and_where(Expr::col(a("id")).eq(Expr::cust("$1")))
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
			sqlx::query(
				&Query::update()
					.table(a("semantic_remote_attempts"))
					.value(
						a("state"),
						Expr::cust(
							"CASE WHEN state = 'RESERVING' THEN 'ABORTED' ELSE 'UNCERTAIN' END",
						),
					)
					.and_where(Expr::cust("id=$1 AND state IN ('RESERVING','DISPATCHED')"))
					.to_string(PostgresQueryBuilder),
			)
			.bind(previous)
			.execute(&mut *tx)
			.await?;
		}
		let failures = record.failures + 1;
		let delay = retry_delay(failures);
		sqlx::query(
			&Query::update()
				.table(a("semantic_remote_operations"))
				.value(a("state"), Expr::cust("$2"))
				.value(a("failures"), failures)
				.value(a("fence"), Expr::col(a("fence")).add(1))
				.value(a("lease_until"), Expr::cust("NULL"))
				.value(
					a("next_attempt"),
					Expr::cust("CLOCK_TIMESTAMP() + MAKE_INTERVAL(secs => $3)"),
				)
				.value(a("error"), Expr::cust("$4"))
				.and_where(Expr::col(a("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.bind(if delay.is_some() { "WAITING" } else { "PAUSED" })
		.bind(delay.map(|s| s as f64))
		.bind(code(if delay.is_some() {
			Failure::Unavailable
		} else {
			Failure::RetriesExhausted
		})?)
		.execute(&mut *tx)
		.await?;
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
	sqlx::query(
		&Query::update()
			.table(a("semantic_remote_operations"))
			.value(a("state"), "ACTIVE")
			.value(a("fence"), attempt.fence)
			.value(a("attempt_id"), Expr::cust("$2"))
			.value(a("next_attempt"), Expr::cust("NULL"))
			.value(
				a("lease_until"),
				Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '120 seconds'"),
			)
			.and_where(Expr::col(a("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(attempt.id)
	.execute(&mut *tx)
	.await?;
	sqlx::query(
		&Query::insert()
			.into_table(a("semantic_remote_attempts"))
			.columns(["id", "operation_id", "fence", "cycle", "state"].map(a))
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				attempt.fence.into(),
				record.cycle.into(),
				"RESERVING".into(),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(attempt.id)
	.bind(id)
	.execute(&mut *tx)
	.await?;
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
	let changed = sqlx::query(
		&Query::update()
			.table(a("semantic_remote_attempts"))
			.value(a("state"), "DISPATCHED")
			.value(a("reservations"), Expr::cust("$2"))
			.value(a("dispatched_at"), Expr::cust("CLOCK_TIMESTAMP()"))
			.and_where(Expr::cust("id=$1 AND state='RESERVING'"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(attempt.id)
	.bind(reservations)
	.execute(&mut *tx)
	.await?
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
		sqlx::query(
			&Query::insert()
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
				.values_panic(["$1", "$2", "$3", "$4", "$5"].map(Expr::cust))
				.on_conflict(OnConflict::new().do_nothing().to_owned())
				.to_string(PostgresQueryBuilder),
		)
		.bind(receipt.grant_id)
		.bind(receipt.admission_id)
		.bind(source.entry_id)
		.bind(source.revision)
		.bind(&source.content_digest)
		.execute(&mut *tx)
		.await?;
	}
	sqlx::query(
		&Query::update()
			.table(a("semantic_remote_operations"))
			.value(a("state"), "READY")
			.value(a("receipt"), Expr::cust("$2"))
			.value(a("lease_until"), Expr::cust("NULL"))
			.value(a("error"), Expr::cust("NULL"))
			.and_where(Expr::col(a("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(receipt.operation_id)
	.bind(serde_json::to_value(receipt)?)
	.execute(&mut *tx)
	.await?;
	sqlx::query(
		&Query::update()
			.table(a("semantic_remote_attempts"))
			.value(a("state"), "COMPLETED")
			.value(a("completed_at"), Expr::cust("CLOCK_TIMESTAMP()"))
			.and_where(Expr::col(a("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(attempt.id)
	.execute(&mut *tx)
	.await?;
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
			.value(a("error"), Expr::cust("$2"))
			.value(a("lease_until"), Expr::cust("NULL"))
			.value(
				a("next_attempt"),
				Expr::cust("CLOCK_TIMESTAMP() + MAKE_INTERVAL(secs => $3)"),
			)
			.and_where(Expr::col(a("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(record.id)
	.bind(code(failure)?)
	.bind(delay.map(|s| s as f64))
	.execute(&mut *tx)
	.await?;
	sqlx::query(
		&Query::update()
			.table(a("semantic_remote_attempts"))
			.value(
				a("state"),
				Expr::cust("CASE WHEN state='RESERVING' THEN 'ABORTED' ELSE 'UNCERTAIN' END"),
			)
			.value(a("error"), Expr::cust("$2"))
			.and_where(Expr::cust("id=$1 AND state IN ('RESERVING','DISPATCHED')"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(attempt.id)
	.bind(code(failure)?)
	.execute(&mut *tx)
	.await?;
	tx.commit().await?;
	Ok(failure)
}

pub(crate) async fn expire_cached(store: &Store, id: Uuid) -> Result<()> {
	sqlx::query(
		&Query::update()
			.table(a("semantic_remote_operations"))
			.value(a("state"), "PENDING")
			.value(a("receipt"), Expr::cust("NULL"))
			.and_where(Expr::cust("id=$1 AND state='READY'"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.execute(&store.pool)
	.await?;
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
		.order_by(a("id"), sea_orm::sea_query::Order::Asc)
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
		sqlx::query(
			&Query::update()
				.table(a("semantic_remote_operations"))
				.value(a("cycle"), Expr::col(a("cycle")).add(1))
				.value(a("state"), "PENDING")
				.value(a("failures"), 0)
				.value(a("next_attempt"), Expr::cust("NULL"))
				.value(a("lease_until"), Expr::cust("NULL"))
				.value(a("error"), Expr::cust("NULL"))
				.and_where(Expr::col(a("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(record.id)
		.execute(&mut **tx)
		.await?;
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
