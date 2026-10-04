//! Compaction use case: classify history, prune, then atomically accept a fitting context.
use crate::{Error, Result};
use aidash_domain::context::{Context, RequestBudget, compaction_snapshot};
use serde_json::{Value, json};
mod compaction;

pub async fn compact(
	context: &mut Context,
	asker: &dyn crate::ports::CompactionClassifier,
	budget: &RequestBudget<'_>,
	pinned: &Value,
) -> Result<()> {
	let size = |c: &Context| budget.request(c, pinned).estimated_total_tokens();
	let mut candidate = context.clone();
	if size(&candidate) <= budget.window {
		*context = candidate;
		return Ok(());
	}
	let classification_context = json!({
		"instructions":budget.instructions, "current":compaction_snapshot(pinned), "previous_summary":candidate.summary
	});
	let compacted = compaction::prune(
		&candidate.history,
		&classification_context,
		asker,
		&compaction::Options::default(),
	)
	.await?;
	candidate.history = compacted.history;
	// No summarization fallback: legacy summaries and all non-tool events stay
	// verbatim. Apply nothing unless the complete inference context fits.
	if size(&candidate) > budget.window {
		return Err(Error::Invalid(
			"Jev compaction could not fit the pinned context and retained history".into(),
		));
	}
	candidate.compactions += 1;
	tracing::info!(
		requests = compacted.requests,
		stage = compacted.stage,
		calls_dropped = compacted.calls_dropped,
		results_truncated = compacted.results_truncated,
		"Jev context compaction completed"
	);
	*context = candidate;
	Ok(())
}

pub fn probability(response: &Value, name: &str) -> Result<f64> {
	let answer = &response["answers"][name];
	let value = answer["noul"]
		.as_f64()
		.filter(|v| v.is_finite() && (0.0..=1.0).contains(v));
	if answer.get("type").is_some_and(|kind| kind != "noul") || value.is_none() {
		return Err(Error::External(format!("invalid Jev answer for {name}")));
	}
	Ok(value.unwrap())
}

#[cfg(test)]
mod tests;
