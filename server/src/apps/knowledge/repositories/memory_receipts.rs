//! All receipt writers compare the winner of a globally keyed reservation.
use super::access::Lease;
use crate::{Error, Result, database::native};
use aidash_domain::memory::Evidence;
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use uuid::Uuid;

/// Return whether this transaction reserved the request, or matched its winner.
/// A conflict must roll back the caller's authority transaction and its effects.
pub(crate) async fn reserve(
	lease: &mut Lease<'_>,
	operation: Uuid,
	bank: Uuid,
	digest: &str,
	outcome: &[Evidence],
	created_at: DateTime<Utc>,
) -> Result<bool> {
	let reservation = native::query(
		&Query::insert()
			.into_table(Alias::new("memory_receipts"))
			.columns(["operation_id", "bank_id", "digest", "outcome", "created_at"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(operation))
					.expr(Expr::value(bank))
					.expr(Expr::value(digest))
					.expr(Expr::value(serde_json::to_value(outcome)?))
					.expr(Expr::value(created_at))
					.to_owned(),
			)
			.on_conflict(
				OnConflict::column(Alias::new("operation_id"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	if reservation.rows_affected() == 1 {
		return Ok(true);
	}
	// Workspace locks are independent, but the receipt key is global. The
	// insert waits for the winner; compare its committed request and outcome.
	let receipt = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_receipts"))
			.and_where(Expr::col("operation_id").eq(Expr::value(operation)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?
	.ok_or_else(|| Error::Conflict("memory operation receipt changed during reservation".into()))?;
	if receipt.try_get::<Uuid>("bank_id")? != bank || receipt.try_get::<String>("digest")? != digest
	{
		return Err(Error::Conflict(
			"memory operation ID was reused with a different request".into(),
		));
	}
	if receipt.try_get::<Vec<Evidence>>("outcome")? != outcome {
		return Err(Error::Conflict(
			"memory operation completed; its result has since changed".into(),
		));
	}
	Ok(false)
}
