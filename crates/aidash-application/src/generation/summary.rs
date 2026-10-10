//! Summary Stage calls require every generated ancestor's exact summarizer approval.
//! Calls are charged before I/O; failed, uncertain and crashed calls stay charged.
use crate::{
	Error, Result,
	authorization::catalog,
	ports::generation::{
		compaction::GenerationCompactionAuthority, summary::GenerationSummaryRepository,
	},
};
use aidash_domain::{
	context::{recovery::Failure, summary::SummaryProvider},
	generation::{compaction::Context, policy::Spec, summary::Attempt},
	registry::rules::digest,
};
use uuid::Uuid;

fn unavailable() -> Error {
	Error::Context(Failure::SummaryUnavailable)
}

/// Return whether a generated chain was charged. Ordinary Runs have no
/// generated ancestors and are authorized by catalog approval alone.
pub async fn reserve(
	scope: &mut dyn GenerationCompactionAuthority,
	repository: &dyn GenerationSummaryRepository,
	context: &Context,
	summarizer: &SummaryProvider,
	request_bytes: i64,
) -> Result<bool> {
	let jobs = scope.ancestors(repository.node_id()).await?;
	if jobs.is_empty() {
		return Ok(false);
	}
	super::publication::require_live(
		scope.live(),
		repository.node_id(),
		context.task,
		&context.agent,
	)
	.await?;
	for (_, document) in &jobs {
		let spec: Spec = serde_json::from_value(document.clone())?;
		// Every ancestor must approve this exact summarizer; none may substitute.
		if spec
			.summary
			.as_ref()
			.is_none_or(|approved| approved.provider != summarizer.model)
		{
			return Err(unavailable());
		}
	}
	catalog::entry(scope.catalog(), &summarizer.model, "registry.read").await?;
	let entry = catalog::entry(scope.catalog(), &summarizer.model, "model.infer").await?;
	if entry.kind != "model"
		|| digest(&serde_json::to_value(&entry)?) != summarizer.definition_digest
	{
		return Err(unavailable());
	}
	let attempt = Attempt {
		id: Uuid::new_v4(),
		run: context.run,
		provider: summarizer.model.clone(),
		definition_digest: summarizer.definition_digest.clone(),
		request_bytes,
	};
	let mut transaction = repository.begin().await?;
	for (id, _) in jobs {
		if !transaction.charge(id).await? {
			return Err(unavailable());
		}
		transaction.reserve(id, &attempt).await?;
	}
	transaction.commit().await?;
	Ok(true)
}

#[cfg(test)]
mod tests;
