//! Provider permission, exact pins and call accounting precede context classification.
use crate::{
	Error, Result,
	authorization::catalog,
	ports::{
		CompactionQuestions,
		generation::compaction::{
			ApprovedCompactionTransport, GenerationCompactionAuthority,
			GenerationCompactionProvider, GenerationCompactionRepository,
		},
	},
};
use aidash_domain::generation::{
	compaction::{Attempt, Context},
	policy::Spec,
};
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

/// Return a validated transport only after the entire ancestor reservation commits.
/// Ordinary agents retain their separately configured compaction transport.
pub async fn reserve(
	scope: &mut dyn GenerationCompactionAuthority,
	repository: &dyn GenerationCompactionRepository,
	provider: &dyn GenerationCompactionProvider,
	context: &Context,
	state: &Value,
	questions: &CompactionQuestions,
) -> Result<Option<Arc<dyn ApprovedCompactionTransport>>> {
	let jobs = scope.ancestors(repository.node_id()).await?;
	if jobs.is_empty() {
		return Ok(None);
	}
	super::publication::require_live(
		scope.live(),
		repository.node_id(),
		context.task,
		&context.agent,
	)
	.await?;
	let mut reference = None;
	for (_, document) in &jobs {
		let spec: Spec = serde_json::from_value(document.clone())?;
		let approved = spec.compaction.ok_or_else(|| {
			Error::Invalid(
				"generated context requires a separately approved compaction provider".into(),
			)
		})?;
		if reference
			.as_ref()
			.is_some_and(|reference| reference != &approved.provider)
		{
			return Err(Error::Forbidden);
		}
		reference = Some(approved.provider);
	}
	let reference = reference.ok_or(Error::Forbidden)?;
	catalog::entry(scope.catalog(), &reference, "registry.read").await?;
	let entry = catalog::entry(scope.catalog(), &reference, "compaction.invoke").await?;
	if entry.kind != "compactor" {
		return Err(Error::Forbidden);
	}
	let transport = provider.approved(serde_json::from_value(entry.config)?)?;
	let bytes = transport.check_request(state, questions)?;
	let attempt = Attempt {
		id: Uuid::new_v4(),
		run: context.run,
		provider: reference,
		request_bytes: bytes as i64,
		questions: questions.len() as i32,
	};
	let mut transaction = repository.begin().await?;
	for (id, _) in jobs {
		if !transaction.charge(id).await? {
			return Err(Error::Invalid(
				"generated compaction call budget exhausted".into(),
			));
		}
		transaction.reserve(id, &attempt).await?;
	}
	transaction.commit().await?;
	// Failed, uncertain and crashed provider calls stay charged. Every retry
	// reserves a new attempt, including retries of the same worker step.
	Ok(Some(transport))
}

#[cfg(test)]
mod tests;

pub mod remote;
