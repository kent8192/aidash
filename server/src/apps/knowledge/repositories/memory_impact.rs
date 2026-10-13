//! Bounded reads of the dependency store for authorized impact inspection.
use super::access::Lease;
use crate::{Result, database::native};
use aidash_domain::RunMetadata;
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

/// Run reads of `unit`, oldest Run first, at most `scan + 1` so a caller can
/// tell that the scan stopped early.
pub(crate) async fn run_reads(
	lease: &mut Lease<'_>,
	unit: Uuid,
	scan: usize,
) -> Result<Vec<(Uuid, i64)>> {
	native::query(
		&Query::select()
			.columns(["run_id", "revision"].map(Alias::new))
			.from(Alias::new("memory_run_reads"))
			.and_where(Expr::col("unit_id").eq(Expr::value(unit)))
			.order_by(Alias::new("run_id"), Order::Asc)
			.order_by(Alias::new("revision"), Order::Asc)
			.limit(scan as u64 + 1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **lease.tx())
	.await?
	.into_iter()
	.map(|row| Ok((row.try_get("run_id")?, row.try_get("revision")?)))
	.collect()
}

pub(crate) async fn run(lease: &mut Lease<'_>, id: Uuid) -> Result<Option<RunMetadata>> {
	crate::database::query_as::<RunMetadata>(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await
}

/// Units derived directly from `source`, at most `cap + 1`.
pub(crate) async fn dependents(
	lease: &mut Lease<'_>,
	source: Uuid,
	cap: usize,
) -> Result<Vec<Uuid>> {
	native::query_scalar(
		&Query::select()
			.distinct()
			.column(Alias::new("unit_id"))
			.from(Alias::new("memory_dependencies"))
			.and_where(Expr::col("source_kind").eq("unit"))
			.and_where(Expr::col("source_id").eq(Expr::value(source)))
			.order_by(Alias::new("unit_id"), Order::Asc)
			.limit(cap as u64 + 1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&mut **lease.tx())
	.await
}
