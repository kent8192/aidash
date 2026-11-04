//! Fenced progress on Node Deletions (ADR 0083). Every write names the
//! deletion's id and requires it to be neither restored nor purged, so a purge
//! worker that lost its lease or met a restore changes nothing.
use crate::apps::identity::models::{NodeDeletion, database_time};
use crate::{Error, Result};
use aidash_domain::nodes::deletion::PurgeStep;
use chrono::{DateTime, Duration, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

/// How long a purge worker owns a purge before another may take it over. A
/// step renews it as it works, so only a crashed worker's lease runs out.
pub(crate) const LEASE: Duration = Duration::minutes(15);
/// The longest error kept for Deployment Administrators to read.
const ERROR_CHARS: usize = 500;

fn is_open(
	statement: &mut reinhardt::query::UpdateStatement,
) -> &mut reinhardt::query::UpdateStatement {
	statement
		.and_where(Expr::col("restored_at").is_null())
		.and_where(Expr::col("purged_at").is_null())
}

/// Writes the open deletion `id`, and, with a `lease`, only while the purge
/// still holds it.
async fn update_one(
	tx: &mut dyn TransactionExecutor,
	mut statement: reinhardt::query::UpdateStatement,
	id: Uuid,
	lease: Option<Uuid>,
) -> Result<()> {
	statement.and_where(Expr::col("id").eq(Expr::value(id)));
	if let Some(lease) = lease {
		statement.and_where(Expr::col("purge_lease").eq(Expr::value(lease)));
		statement
			.and_where(Expr::col("purge_leased_until").gt(Expr::value(database_time(tx).await?)));
	}
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
		.table(Alias::new(NodeDeletion::table_name()))
		.to_owned()
}

/// Claims one candidate in its owning Tenant after policy/capacity locks.
/// Both deadline expiry and an explicit API request start irreversible purge.
pub(crate) async fn claim_one(
	tx: &mut dyn TransactionExecutor,
	candidate: &NodeDeletion,
) -> Result<Option<NodeDeletion>> {
	let Some(mut row) = NodeDeletion::objects()
		.filter(NodeDeletion::field_id().eq(candidate.id))
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop()
	else {
		return Ok(None);
	};
	let now = database_time(tx).await?;
	if row.restored_at.is_some()
		|| row.purged_at.is_some()
		|| (row.purge_step.is_none() && row.purge_after > now)
		|| row.purge_leased_until.is_some_and(|until| until > now)
	{
		return Ok(None);
	}
	let lease = Uuid::new_v4();
	let (sql, values) = update()
		.value_expr(Alias::new("purge_lease"), Expr::value(lease))
		.value_expr(Alias::new("purge_leased_until"), Expr::value(now + LEASE))
		.value_expr(
			Alias::new("purge_step"),
			Expr::value(
				row.purge_step
					.clone()
					.unwrap_or_else(|| PurgeStep::FIRST.as_str().into()),
			),
		)
		.and_where(Expr::col("id").eq(Expr::value(row.id)))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	row.purge_lease = Some(lease);
	row.purge_leased_until = Some(now + LEASE);
	row.purge_step = Some(
		row.purge_step
			.unwrap_or_else(|| PurgeStep::FIRST.as_str().into()),
	);
	Ok(Some(row))
}

/// Extends the lease of a purge that is still working.
pub(crate) async fn renew(tx: &mut dyn TransactionExecutor, deletion: &NodeDeletion) -> Result<()> {
	let now = database_time(tx).await?;
	let mut statement = update();
	statement.value_expr(Alias::new("purge_leased_until"), Expr::value(now + LEASE));
	update_one(tx, statement, deletion.id, deletion.purge_lease).await
}

/// Locks the deletion row and checks, under that lock, that it is still
/// unrestored, unpurged and leased to this purge. The caller keeps the lock
/// until its transaction ends, so a restore cannot land between this check and
/// what the step then does; a restore that waits for the lock finds the lease
/// held and is refused.
pub(crate) async fn hold(
	tx: &mut dyn TransactionExecutor,
	deletion: &NodeDeletion,
) -> Result<Option<String>> {
	let row = NodeDeletion::objects()
		.filter(NodeDeletion::field_id().eq(deletion.id))
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop();
	let now = database_time(tx).await?;
	if row.as_ref().is_none_or(|row| {
		row.tenant != deletion.tenant
			|| row.node != deletion.node
			|| row.restored_at.is_some()
			|| row.purged_at.is_some()
			|| deletion.purge_lease.is_none()
			|| row.purge_lease != deletion.purge_lease
			|| row.purge_leased_until.is_none_or(|until| until <= now)
	}) {
		return Err(Error::Conflict(
			"Node purge no longer holds an open, unexpired deletion lease".into(),
		));
	}
	Ok(row.and_then(|row| row.purge_step))
}

/// Records that the purge finished every step before `next`, clearing what an
/// earlier failure left.
pub(crate) async fn advance(
	tx: &mut dyn TransactionExecutor,
	deletion: &NodeDeletion,
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
	update_one(tx, statement, deletion.id, deletion.purge_lease).await
}

/// Records that `step` failed, and releases the purge for the next sweep.
pub(crate) async fn fail(
	tx: &mut dyn TransactionExecutor,
	deletion: &NodeDeletion,
	step: PurgeStep,
	error: &str,
) -> Result<()> {
	let now = database_time(tx).await?;
	let error = error.chars().take(ERROR_CHARS).collect::<String>();
	let mut statement = update();
	statement.and_where(Expr::col("purge_step").eq(Expr::value(step.as_str())));
	statement
		.value_expr(Alias::new("purge_step"), Expr::value(step.as_str()))
		.value_expr(
			Alias::new("purge_failures"),
			Expr::col("purge_failures").add(Expr::value(1_i64)),
		)
		.value_expr(Alias::new("purge_error"), Expr::value(error))
		.value_expr(Alias::new("purge_failed_at"), Expr::value(now))
		.value_expr(Alias::new("purge_lease"), Expr::value(None::<Uuid>))
		.value_expr(
			Alias::new("purge_leased_until"),
			Expr::value(None::<DateTime<Utc>>),
		);
	update_one(tx, statement, deletion.id, deletion.purge_lease).await
}

/// Clears the label on the root record and marks the deletion purged. The
/// name, creation time and creator stay as the tombstone (ADR 0050).
pub(crate) async fn finish(
	tx: &mut dyn TransactionExecutor,
	deletion: &NodeDeletion,
) -> Result<()> {
	let now = database_time(tx).await?;
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
		.value_expr(Alias::new("purge_lease"), Expr::value(None::<Uuid>))
		.value_expr(
			Alias::new("purge_leased_until"),
			Expr::value(None::<DateTime<Utc>>),
		);
	update_one(tx, statement, deletion.id, deletion.purge_lease).await
}
