//! A record that a Journal range was admitted into memory, so the range can
//! expire from the Journal without breaking the memory that cites it.
use super::access::Lease;
use crate::{Error, Result, database::native};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoValue, LockType, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use uuid::Uuid;

fn id(run: Uuid, from: u64, through: u64) -> Result<Uuid> {
	crate::semantic::native_memory::request_id(run, &format!("journal-proof:{from}:{through}"))
}

pub(crate) async fn record(
	lease: &mut Lease<'_>,
	tenant: &str,
	run: Uuid,
	from: u64,
	through: u64,
) -> Result<()> {
	let key = id(run, from, through)?;
	if load(lease, run, from, through).await?.is_some() {
		return Ok(());
	}
	let start =
		i64::try_from(from).map_err(|_| Error::Invalid("Journal sequence overflow".into()))?;
	let end =
		i64::try_from(through).map_err(|_| Error::Invalid("Journal sequence overflow".into()))?;
	let rows = native::query(
		&Query::select()
			.columns(["seq", "event", "digest"].map(Alias::new))
			.from(Alias::new(
				crate::apps::execution::repositories::context_journal::EVENTS,
			))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("seq").gte(Expr::value(start)))
			.and_where(Expr::col("seq").lte(Expr::value(end)))
			.order_by(Alias::new("seq"), Order::Asc)
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **lease.tx())
	.await?;
	if through.checked_sub(from).and_then(|n| n.checked_add(1)) != Some(rows.len() as u64) {
		return Err(Error::Conflict(
			"memory Journal range removed before admission".into(),
		));
	}
	for (offset, row) in rows.iter().enumerate() {
		let seq: i64 = row.try_get("seq")?;
		let event: serde_json::Value = row.try_get("event")?;
		let digest: String = row.try_get("digest")?;
		if seq != start + offset as i64 || aidash_domain::registry::rules::digest(&event) != digest
		{
			return Err(Error::Conflict("memory Journal range changed".into()));
		}
	}
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_journal_proofs"))
			.columns(["id", "tenant", "run_id", "from_seq", "through_seq"].map(Alias::new))
			.values_panic([
				key.into_value(),
				tenant.into_value(),
				run.into_value(),
				start.into_value(),
				end.into_value(),
			])
			.on_conflict(OnConflict::column(Alias::new("id")).do_nothing().to_owned())
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	if load(lease, run, from, through).await?.is_none() {
		return Err(Error::Conflict("memory Journal proof collision".into()));
	}
	Ok(())
}

pub(crate) async fn load(
	lease: &mut Lease<'_>,
	run: Uuid,
	from: u64,
	through: u64,
) -> Result<Option<()>> {
	let row = native::query(
		&Query::select()
			.columns(["run_id", "from_seq", "through_seq"].map(Alias::new))
			.from(Alias::new("memory_journal_proofs"))
			.and_where(Expr::col("id").eq(Expr::value(id(run, from, through)?)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?;
	let Some(row) = row else {
		return Ok(None);
	};
	let saved_run: Uuid = row.try_get("run_id")?;
	let saved_from: i64 = row.try_get("from_seq")?;
	let saved_through: i64 = row.try_get("through_seq")?;
	if saved_run != run
		|| u64::try_from(saved_from).ok() != Some(from)
		|| u64::try_from(saved_through).ok() != Some(through)
	{
		return Err(Error::Conflict("memory Journal proof collision".into()));
	}
	Ok(Some(()))
}
