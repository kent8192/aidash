//! Automatic memory read receipts commit only with the adopted inference.
use crate::{Error, Result, database::native, store::Store};
use aidash_domain::{Run, context::source::Completeness, memory::Unit, registry::rules::digest};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

pub(super) async fn accept(
	store: &Store,
	tx: &mut native::Transaction,
	run: &Run,
	worker: Uuid,
) -> Result<()> {
	if !run.is_local(store.deployment.as_str()) || run.context.source_deliveries.is_empty() {
		return Ok(());
	}
	// save_run_in has already locked/fenced and updated this Run. A receipt
	// from a previous attempt cannot grant a new automatic full-read here.
	let attempts: Vec<Uuid> = native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("inference_attempts"))
			.and_where(Expr::col("run_id").eq(Expr::value(run.id)))
			.and_where(Expr::col("usage_attempt").eq(Expr::value(worker)))
			.and_where(Expr::col("outcome").is_null())
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&mut **tx)
	.await?;
	let mut units = vec![];
	for receipt in &run.context.source_deliveries {
		if !attempts.contains(&receipt.accepted_inference) {
			continue;
		}
		let contribution = run
			.context
			.source_contributions
			.iter()
			.find(|contribution| contribution.identity == receipt.identity)
			.ok_or_else(|| {
				Error::Invalid("source delivery lacks its fetched contribution".into())
			})?;
		if contribution.content_digest != digest(&contribution.content)
			|| receipt.content_digest != contribution.content_digest
			|| contribution.completeness != receipt.completeness
		{
			return Err(Error::Invalid(
				"accepted source delivery commitment changed".into(),
			));
		}
		if contribution
			.placement
			.as_ref()
			.is_none_or(|placement| placement.slot != "semantic_memory")
			|| contribution.completeness != Completeness::Complete
		{
			continue;
		}
		if contribution.fetched_digest != contribution.content_digest {
			return Err(Error::Invalid(
				"partial memory content cannot grant a full read".into(),
			));
		}
		if let Ok(unit) = serde_json::from_value::<Unit>(contribution.content.clone())
			&& !units
				.iter()
				.any(|other: &Unit| other.id == unit.id && other.revision == unit.revision)
		{
			units.push(unit);
		}
	}
	if !units.is_empty() {
		crate::semantic::services::memory_context::accept(
			store,
			&mut crate::semantic::service::Lease::BorrowedWithoutSubject(tx),
			run,
			&units,
		)
		.await?;
	}
	Ok(())
}
