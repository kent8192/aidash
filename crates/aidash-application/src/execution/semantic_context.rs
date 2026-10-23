//! Agent context retains input order, remote boundary refresh and durable local dependencies.
use crate::{
	Result, ports::semantic::run_context::RunSemanticRepository,
	semantic::retrieval::WorkspaceContext,
};
use aidash_domain::{
	Task,
	semantic::{InputRead, degraded::marker},
};
use serde_json::Value;
/// `degrade`: a local workspace outage yields the Degraded context marker
/// instead of an error (ADR 0059). A remote Run never degrades.
pub async fn retrieve(
	repository: &dyn RunSemanticRepository,
	task: &Task,
	inputs: &[(InputRead, String)],
	budget: usize,
	degrade: bool,
) -> Result<Option<Value>> {
	let mut query = format!("{}\n{}", task.title, task.description);
	for (_, text) in inputs {
		query.push('\n');
		query.push_str(text);
	}
	retrieve_with_query(repository, task, inputs, &query, budget, degrade).await
}
pub async fn retrieve_with_query(
	repository: &dyn RunSemanticRepository,
	task: &Task,
	inputs: &[(InputRead, String)],
	query: &str,
	budget: usize,
	degrade: bool,
) -> Result<Option<Value>> {
	if repository.remote() {
		repository.suspend().await?;
		let result = repository
			.remote_context(task, inputs, query, budget)
			.await?;
		repository.refresh_remote().await?;
		return Ok(result);
	}
	let mut scope = repository.local_scope().await?;
	let result = match scope.retrieve(query, budget).await? {
		// A degraded slot read no entry, so it begins no dependency journal.
		WorkspaceContext::Unavailable(cause) if degrade => return Ok(Some(marker(cause))),
		workspace => workspace.required()?,
	};
	if let Some(result) = &result {
		let mut journal = scope.begin_dependencies().await?;
		for matched in &result.matches {
			journal.record(matched.entry_id, matched.revision).await?;
		}
		journal.commit().await?;
	}
	result
		.map(serde_json::to_value)
		.transpose()
		.map_err(Into::into)
}
#[cfg(test)]
mod tests;

/// One explicitly selected policy governs the native combined semantic envelope.
/// Implicit defaults inherit it; contradictory explicit declarations fail admission
/// of the read rather than silently querying two sources with different corrections.
pub fn query_material(
	run: &aidash_domain::Run,
	task: &Task,
	inputs: &[(InputRead, String)],
	max_bytes: usize,
) -> Result<aidash_domain::context::source::QueryMaterial> {
	use aidash_domain::{
		context::source::{self, AcceptedQueryInput, QueryPolicy},
		registry::bindings::sources::{NativeContext, NativeSource},
	};
	let mut policy = None;
	if let Some(snapshot) = &run.context.binding_snapshot {
		for binding in snapshot.bindings.iter().filter(|binding| {
			binding.excluded_reason.is_none()
				&& matches!(binding.definition.kind.as_str(), "memory" | "source")
		}) {
			let explicit = if let Ok(native) =
				serde_json::from_value::<NativeContext>(binding.definition.config.clone())
			{
				if matches!(
					native.source,
					NativeSource::ConversationMemory { .. }
						| NativeSource::SemanticMemory { .. }
						| NativeSource::WorkspaceRetrieval { .. }
				) {
					native.context_source.map(|declaration| declaration.query)
				} else {
					None
				}
			} else {
				serde_json::from_value::<aidash_domain::memory::SourceConfig>(
					binding.definition.config.clone(),
				)
				.ok()
				.and_then(|source| source.context_source.map(|declaration| declaration.query))
			};
			if let Some(explicit) = explicit {
				if policy.is_some_and(|prior| prior != explicit) {
					return Err(crate::Error::Invalid(
						"native semantic Sources declare conflicting query policies".into(),
					));
				}
				policy = Some(explicit);
			}
		}
	}
	let policy = policy.unwrap_or(QueryPolicy::TaskInputsV1);
	let accepted = inputs
		.iter()
		.map(|(read, text)| AcceptedQueryInput {
			sequence: read.sequence,
			text: text.clone(),
		})
		.collect::<Vec<_>>();
	Ok(source::query_material(
		policy,
		&format!("{}\n{}", task.title, task.description),
		&accepted,
		max_bytes,
	))
}
