//! Context adapters around the application compaction use case.
pub(crate) use super::context_rules::observation;
pub use super::context_rules::{
	Context, ContextEvent, ContextUsage, MessageReadCoverage, RequestBudget, bound_snapshot,
	compaction_snapshot, estimated_tokens, request_context_budget,
};
use crate::Result;
use serde_json::Value;
pub mod jev;

struct Classifier<'a>(&'a dyn jev::JevAsker);
#[async_trait::async_trait]
impl aidash_application::ports::CompactionClassifier for Classifier<'_> {
	async fn ask(
		&self,
		state: &Value,
		questions: &aidash_application::ports::CompactionQuestions,
	) -> aidash_application::Result<Value> {
		self.0.ask(state, questions).await.map_err(Into::into)
	}
}

/// Legacy prune-only compaction for callers without a Context Policy.
pub async fn compact(
	context: &mut Context,
	asker: &dyn jev::JevAsker,
	budget: &RequestBudget<'_>,
	pinned: &Value,
) -> Result<()> {
	use aidash_application::context::{Compaction, Fitting};
	let mut candidate = context.clone();
	observation::normalize_history(&mut candidate.history);
	let policy = aidash_domain::context::policy::Effective::of(None);
	let fitting = Fitting {
		budget,
		pinned,
		policy: &policy,
	};
	match aidash_application::context::compact(&mut candidate, &Classifier(asker), &fitting).await?
	{
		Compaction::Fits => {}
		// A prune-only policy has no Summary Stage.
		Compaction::NeedsSummary(_) => {
			return Err(aidash_application::Error::Context(
				aidash_domain::context::recovery::Failure::ContextUnreducible,
			)
			.into());
		}
	}
	*context = candidate;
	Ok(())
}

#[cfg(test)]
#[path = "../tests/context.rs"]
mod tests;
