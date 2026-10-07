//! Canonical source revisions are independent of disposable search generations.
use super::{access::Lease, bindings, units};
use crate::{Error, Result, database::native, store::Store};
use aidash_domain::{
	RunMetadata,
	memory::{Evidence, Unit},
};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockType, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use uuid::Uuid;
const MAX_READS: usize = 1024;

pub(crate) async fn record(store: &Store, run: Uuid, selected: &[Unit]) -> Result<()> {
	let mut tx = native::begin(&store.control_pool).await?;
	// Separate gate avoids upgrading the binding lock held by the calling scope.
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_run_read_gates"))
			.columns([Alias::new("run_id")])
			.from_subquery(Query::select().expr(Expr::value(run)).to_owned())
			.on_conflict(
				OnConflict::column(Alias::new("run_id"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	native::query(
		&Query::select()
			.column(Alias::new("run_id"))
			.from(Alias::new("memory_run_read_gates"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *tx)
	.await?;
	for unit in selected {
		native::query(
			&Query::insert()
				.into_table(Alias::new("memory_run_reads"))
				.columns(["run_id", "unit_id", "revision"].map(Alias::new))
				.from_subquery(
					Query::select()
						.expr(Expr::value(run))
						.expr(Expr::value(unit.id))
						.expr(Expr::value(unit.revision))
						.to_owned(),
				)
				.on_conflict(
					OnConflict::columns([
						Alias::new("run_id"),
						Alias::new("unit_id"),
						Alias::new("revision"),
					])
					.do_nothing()
					.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
	}
	let rows = native::query(
		&Query::select()
			.column(Alias::new("unit_id"))
			.from(Alias::new("memory_run_reads"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.limit(MAX_READS as u64 + 1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut *tx)
	.await?;
	if rows.len() > MAX_READS {
		return Err(Error::Conflict(
			"Run memory dependency limit reached".into(),
		));
	}
	tx.commit().await
}

pub(crate) async fn visible(lease: &mut Lease<'_>, run_id: Uuid) -> Result<bool> {
	let Some(run) = crate::database::query_as::<RunMetadata>(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(run_id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?
	else {
		return Ok(false);
	};
	let binding = match bindings::load(&mut **lease.tx(), &run).await {
		Err(Error::Conflict(_) | Error::Forbidden) => return Ok(false),
		result => result?,
	};
	let reads = native::query(
		&Query::select()
			.columns(["unit_id", "revision"].map(Alias::new))
			.from(Alias::new("memory_run_reads"))
			.and_where(Expr::col("run_id").eq(Expr::value(run_id)))
			.order_by(Alias::new("unit_id"), Order::Asc)
			.order_by(Alias::new("revision"), Order::Asc)
			.limit(MAX_READS as u64 + 1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **lease.tx())
	.await?;
	if reads.len() > MAX_READS || (!reads.is_empty() && binding.is_none()) {
		return Ok(false);
	}
	if let Some(binding) = binding {
		match units::authorize(lease, &binding.bank, "memory.read").await {
			Err(Error::Forbidden | Error::Conflict(_)) => return Ok(false),
			result => result?,
		}
	}
	for read in reads {
		let Some(unit) = units::load(lease, read.try_get("unit_id")?, false).await? else {
			return Ok(false);
		};
		if !unit.visible()
			|| unit.revision != read.try_get::<i64>("revision")?
			|| unit.bank.workspace != run.workspace_id
		{
			return Ok(false);
		}
		let policy = match units::unexpired(lease, &unit).await {
			Err(Error::Conflict(_) | Error::Forbidden) => return Ok(false),
			result => result?,
		};
		match units::current(
			lease,
			run.workspace_id,
			&[Evidence::Unit {
				bank: unit.bank.clone(),
				id: unit.id,
				revision: unit.revision,
			}],
			policy.bounds.max_graph_visits,
		)
		.await
		{
			Err(Error::Conflict(_) | Error::Forbidden) => return Ok(false),
			result => result?,
		}
	}
	Ok(true)
}
