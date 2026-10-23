//! Deterministic source assembly after authorization and (optional) classification.
use crate::{Error, Result, ports::context_source::ContextSourceProvider};
use aidash_domain::context::source::*;
use aidash_domain::registry::rules::digest;
use serde_json::json;
use std::collections::BTreeMap;

pub async fn fetch(
	provider: &dyn ContextSourceProvider,
	request: &SourceRequest,
) -> Result<SourceBatch> {
	request.declaration.validate()?;
	if matches!(request.declaration.pin, PinPolicy::OnDemand) {
		return Ok(SourceBatch {
			candidates: vec![],
			outcome: SourceOutcome::Excluded,
		});
	}
	let batch = provider.fetch(request).await?;
	batch.validate(&request.source, &request.declaration, request.max_bytes)?;
	Ok(batch)
}
/// Dedup identity+revision+range only. Equal text from different sources stays distinct.
/// Merge every authority dependency; conflicting bytes under one identity fail closed.
pub fn deduplicate(candidates: Vec<SourceCandidate>) -> Result<Vec<SourceCandidate>> {
	let mut seen = BTreeMap::<String, usize>::new();
	let mut output: Vec<SourceCandidate> = vec![];
	for candidate in candidates {
		let key = candidate.identity.key();
		if let Some(index) = seen.get(&key) {
			let prior = &mut output[*index];
			if prior.content != candidate.content
				|| prior.completeness != candidate.completeness
				|| prior.detail != candidate.detail
			{
				return Err(Error::Invalid(
					"source identity resolved to conflicting revision bytes".into(),
				));
			}
			for dependency in candidate.dependencies {
				if !prior.dependencies.contains(&dependency) {
					prior.dependencies.push(dependency);
				}
			}
			for pin in candidate.provenance {
				if !prior.provenance.contains(&pin) {
					prior.provenance.push(pin);
				}
			}
			prior.mandatory |= candidate.mandatory;
		} else {
			seen.insert(key, output.len());
			output.push(candidate);
		}
	}
	Ok(output)
}
#[derive(Debug, Clone)]
pub struct FittedSources {
	pub contributions: Vec<SourceContribution>,
	pub deferred: Vec<CandidateIdentity>,
	pub digest: String,
}
/// Whole attributed envelopes share one final budget. Mandatory receipts never silently drop.
/// A caller with a projection-specific estimator should use `fit_with`.
pub fn fit_contributions(
	candidates: Vec<SourceCandidate>,
	max_bytes: usize,
) -> Result<FittedSources> {
	fit_with(candidates, |items| {
		Ok(serde_json::to_vec(items)?.len() <= max_bytes)
	})
}
pub fn fit_with(
	candidates: Vec<SourceCandidate>,
	fits: impl Fn(&[SourceContribution]) -> Result<bool>,
) -> Result<FittedSources> {
	let candidates = deduplicate(candidates)?;
	let mut contributions = vec![];
	let mut deferred = vec![];
	// Mandatory obligations have priority; output is restored to declaration order below.
	let mut selected = BTreeMap::new();
	for (index, candidate) in candidates
		.iter()
		.enumerate()
		.filter(|(_, c)| c.mandatory)
		.chain(candidates.iter().enumerate().filter(|(_, c)| !c.mandatory))
	{
		let contribution = SourceContribution::from(candidate.clone());
		let mut proposal = selected.clone();
		proposal.insert(index, contribution);
		let ordered = proposal.values().cloned().collect::<Vec<_>>();
		if fits(&ordered)? {
			selected = proposal;
		} else if candidate.mandatory {
			return Err(Error::Invalid(
				"mandatory source contribution cannot fit final request".into(),
			));
		} else {
			deferred.push(candidate.identity.clone());
		}
	}
	contributions.extend(selected.into_values());
	let digest = digest(&json!(contributions));
	Ok(FittedSources {
		contributions,
		deferred,
		digest,
	})
}
