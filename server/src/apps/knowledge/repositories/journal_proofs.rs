//! Immutable Research identities for an admitted Journal range.
use super::access::Lease;
use crate::{Error, Result, database::native};
use aidash_domain::context::research::ResearchReference;
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
	let mut references = Vec::new();
	for (offset, row) in rows.iter().enumerate() {
		let seq: i64 = row.try_get("seq")?;
		let event: serde_json::Value = row.try_get("event")?;
		let digest: String = row.try_get("digest")?;
		if seq != start + offset as i64 || aidash_domain::registry::rules::digest(&event) != digest
		{
			return Err(Error::Conflict("memory Journal range changed".into()));
		}
		if let Some(result) = event.get("result") {
			let reference = ResearchReference::parse(result).or_else(|| {
				ResearchReference::from_result(
					&format!(
						"journal:{seq}:{}",
						event["call"]["id"].as_str().unwrap_or("")
					),
					result,
				)
			});
			if let Some(reference) = reference {
				references.push(reference);
			} else if result["contract"] == aidash_domain::context::research::CONTRACT
				|| result["contract"] == aidash_domain::research::RESULT_CONTRACT
			{
				return Err(Error::Invalid(
					"invalid Research provenance in Journal".into(),
				));
			}
		}
	}
	if !crate::apps::execution::repositories::research::retained_references_current(
		lease.tx().as_mut(),
		run,
		&references,
	)
	.await?
	{
		return Err(Error::Conflict(
			"memory research evidence expired or was revoked".into(),
		));
	}
	let research = serde_json::to_value(references)?;
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_journal_proofs"))
			.columns(
				[
					"id",
					"tenant",
					"run_id",
					"from_seq",
					"through_seq",
					"research",
				]
				.map(Alias::new),
			)
			.values_panic([
				key.into_value(),
				tenant.into_value(),
				run.into_value(),
				start.into_value(),
				end.into_value(),
				research.into_value(),
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
) -> Result<Option<Vec<ResearchReference>>> {
	let row = native::query(
		&Query::select()
			.columns(["run_id", "from_seq", "through_seq", "research"].map(Alias::new))
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
	Ok(Some(serde_json::from_value(row.try_get("research")?)?))
}
