//! The purge state kept on Tenant Deletions (ADR 0061). Every write names the
//! deletion's id and requires it to be neither restored nor purged, so a purge
//! worker that lost its lease or met a restore changes nothing.
use crate::apps::identity::models::{Tenant, TenantDeletion, database_time};
use crate::{Error, Result};
use aidash_domain::identity::tenancy::deletion::{self, PurgeStep};
use chrono::{DateTime, Duration, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, OrmExecutor};
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

/// How long a purge worker owns a purge before another may take it over. A
/// step renews it as it works, so only a crashed worker's lease runs out.
pub(crate) const LEASE: Duration = Duration::minutes(15);
/// Deletions one sweep claims, so one sweep stays bounded.
const CLAIM_BATCH: u64 = 20;
/// The longest error kept for Deployment Administrators to read.
const ERROR_CHARS: usize = 500;

fn is_open(
	statement: &mut reinhardt::query::UpdateStatement,
) -> &mut reinhardt::query::UpdateStatement {
	statement
		.and_where(Expr::col("restored_at").is_null())
		.and_where(Expr::col("purged_at").is_null())
}

async fn update_one(
	tx: &mut dyn TransactionExecutor,
	mut statement: reinhardt::query::UpdateStatement,
	id: Uuid,
) -> Result<()> {
	statement.and_where(Expr::col("id").eq(Expr::value(id)));
	let (sql, values) = is_open(&mut statement).build(PostgresQueryBuilder);
	if tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		!= 1
	{
		return Err(Error::Conflict("the purge is no longer open".into()));
	}
	Ok(())
}

fn update() -> reinhardt::query::UpdateStatement {
	Query::update()
		.table(Alias::new(TenantDeletion::table_name()))
		.to_owned()
}

/// Takes the deletions whose grace period has ended and that no worker owns,
/// oldest deadline first, and leases them to the caller.
pub(crate) async fn claim(tx: &mut dyn TransactionExecutor) -> Result<Vec<TenantDeletion>> {
	let now = database_time(tx).await?;
	let (sql, values) = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new(TenantDeletion::table_name()))
		.and_where(Expr::col("restored_at").is_null())
		.and_where(Expr::col("purged_at").is_null())
		.and_where(Expr::col("purge_after").lte(Expr::value(now)))
		.and_where(
			Expr::col("purge_leased_until")
				.is_null()
				.or(Expr::col("purge_leased_until").lte(Expr::value(now))),
		)
		.order_by(Alias::new("purge_after"), Order::Asc)
		.limit(CLAIM_BATCH)
		.lock(LockType::Update)
		.build(PostgresQueryBuilder);
	let ids = tx
		.fetch_all(&sql, convert_values(values))
		.await?
		.into_iter()
		.map(|row| row.get::<Uuid>("id"))
		.collect::<std::result::Result<Vec<_>, _>>()
		.map_err(reinhardt::core::exception::Error::from)?;
	if ids.is_empty() {
		return Ok(Vec::new());
	}
	let (sql, values) = update()
		.value_expr(Alias::new("purge_leased_until"), Expr::value(now + LEASE))
		.and_where(Expr::col("id").is_in(ids.iter().map(|id| Expr::value(*id))))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(TenantDeletion::objects()
		.filter(TenantDeletion::field_id().is_in(ids))
		.order_by(&["purge_after", "id"])
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?)
}

/// Extends the lease of a purge that is still working.
pub(crate) async fn renew(tx: &mut dyn TransactionExecutor, id: Uuid) -> Result<()> {
	let now = database_time(tx).await?;
	let mut statement = update();
	statement.value_expr(Alias::new("purge_leased_until"), Expr::value(now + LEASE));
	update_one(tx, statement, id).await
}

/// Records that the purge finished every step before `next`, clearing what an
/// earlier failure left.
pub(crate) async fn advance(
	tx: &mut dyn TransactionExecutor,
	id: Uuid,
	next: PurgeStep,
) -> Result<()> {
	let now = database_time(tx).await?;
	let mut statement = update();
	statement
		.value_expr(Alias::new("purge_step"), Expr::value(next.as_str()))
		.value_expr(Alias::new("purge_failures"), Expr::value(0_i64))
		.value_expr(Alias::new("purge_error"), Expr::value(None::<String>))
		.value_expr(
			Alias::new("purge_failed_at"),
			Expr::value(None::<DateTime<Utc>>),
		)
		.value_expr(Alias::new("purge_leased_until"), Expr::value(now + LEASE));
	update_one(tx, statement, id).await
}

/// Records that `step` failed, and releases the purge for the next sweep.
pub(crate) async fn fail(
	tx: &mut dyn TransactionExecutor,
	id: Uuid,
	step: PurgeStep,
	error: &str,
) -> Result<()> {
	let now = database_time(tx).await?;
	let error = error.chars().take(ERROR_CHARS).collect::<String>();
	let mut statement = update();
	statement
		.value_expr(Alias::new("purge_step"), Expr::value(step.as_str()))
		.value_expr(
			Alias::new("purge_failures"),
			Expr::cust("purge_failures + 1"),
		)
		.value_expr(Alias::new("purge_error"), Expr::value(error))
		.value_expr(Alias::new("purge_failed_at"), Expr::value(now))
		.value_expr(
			Alias::new("purge_leased_until"),
			Expr::value(None::<DateTime<Utc>>),
		);
	update_one(tx, statement, id).await
}

/// Clears the label on the root record and marks the deletion purged. The
/// name, creation time and creator stay as the tombstone (ADR 0050).
pub(crate) async fn finish(
	tx: &mut dyn TransactionExecutor,
	deletion: &TenantDeletion,
) -> Result<()> {
	let now = database_time(tx).await?;
	let (sql, values) = Query::update()
		.table(Alias::new(Tenant::table_name()))
		.value_expr(Alias::new("label"), Expr::value(None::<String>))
		.and_where(Expr::col("name").eq(Expr::value(deletion.tenant.clone())))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	let mut statement = update();
	statement
		.value_expr(Alias::new("purged_at"), Expr::value(now))
		.value_expr(Alias::new("purge_step"), Expr::value(None::<String>))
		.value_expr(Alias::new("purge_failures"), Expr::value(0_i64))
		.value_expr(Alias::new("purge_error"), Expr::value(None::<String>))
		.value_expr(
			Alias::new("purge_failed_at"),
			Expr::value(None::<DateTime<Utc>>),
		)
		.value_expr(
			Alias::new("purge_leased_until"),
			Expr::value(None::<DateTime<Utc>>),
		);
	update_one(tx, statement, deletion.id).await
}

/// Ends the grace period of `deletion` now, so the next sweep purges it. The
/// caller has locked the deletion and checked it is still restorable.
pub(crate) async fn purge_now(
	tx: &mut dyn TransactionExecutor,
	deletion: &TenantDeletion,
) -> Result<TenantDeletion> {
	let now = database_time(tx).await?;
	// The deadline stays after the deletion, as the table's check requires.
	let at = now
		.max(deletion.deleted_at + Duration::microseconds(1))
		.min(deletion.purge_after);
	let mut statement = update();
	statement.value_expr(Alias::new("purge_after"), Expr::value(at));
	update_one(tx, statement, deletion.id).await?;
	Ok(TenantDeletion {
		purge_after: at,
		..deletion.clone()
	})
}

/// The purged deletions whose audit history has been kept for `retention_days`
/// and not yet deleted, oldest first.
pub(crate) async fn audit_due<E: OrmExecutor>(
	tx: &mut E,
	now: DateTime<Utc>,
	retention_days: u32,
) -> Result<Vec<TenantDeletion>> {
	Ok(TenantDeletion::objects()
		.filter(
			TenantDeletion::field_purged_at()
				.lte(Some(now - Duration::days(i64::from(retention_days)))),
		)
		.filter(TenantDeletion::field_audit_erased_at().is_null())
		.order_by(&["purged_at", "id"])
		.limit(CLAIM_BATCH as usize)
		.all_with_db(tx)
		.await?)
}

/// Marks the audit history of a purged Tenant deleted.
pub(crate) async fn audit_erased(tx: &mut dyn TransactionExecutor, id: Uuid) -> Result<()> {
	let now = database_time(tx).await?;
	let (sql, values) = update()
		.value_expr(Alias::new("audit_erased_at"), Expr::value(now))
		.and_where(Expr::col("id").eq(Expr::value(id)))
		.and_where(Expr::col("purged_at").is_not_null())
		.and_where(Expr::col("audit_erased_at").is_null())
		.build(PostgresQueryBuilder);
	if tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		!= 1
	{
		return Err(Error::Conflict(
			"the audit history is already deleted".into(),
		));
	}
	Ok(())
}

/// Whether `failures` consecutive failures make the purge stalled.
pub(crate) fn stalled(deletion: &TenantDeletion) -> bool {
	deletion::stalled(u32::try_from(deletion.purge_failures).unwrap_or(u32::MAX))
}
