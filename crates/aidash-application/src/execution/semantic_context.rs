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

/// Native combined retrieval has one query. Admission requires every effective
/// source policy to agree, including declarations that keep the default.
pub fn query_material(
	run: &aidash_domain::Run,
	task: &Task,
	inputs: &[(InputRead, String)],
	max_bytes: usize,
) -> Result<aidash_domain::context::source::QueryMaterial> {
	use aidash_domain::context::source::{self, AcceptedQueryInput, QueryPolicy};
	let policy = run
		.context
		.binding_snapshot
		.as_deref()
		.map(aidash_domain::registry::bindings::sources::semantic_query_policy)
		.transpose()?
		.unwrap_or(QueryPolicy::TaskInputsV1);
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

/// The native combined provider's reusable read key. The operator delegates
/// this material to the environment so other providers can use their own keys.
pub fn retrieval_key(
	run: &aidash_domain::Run,
	bindings: &str,
	frame: crate::ports::context_source::ContextSourceFrame<'_>,
	scope: aidash_domain::context::sources::RetrievalScope,
) -> Result<aidash_domain::context::sources::RetrievalKey> {
	Ok(aidash_domain::context::sources::RetrievalKey::new(
		run.id,
		frame.task,
		frame.inputs.iter().map(|(read, _)| read.clone()).collect(),
		frame.budget,
		bindings.into(),
		scope,
	)
	.with_query_material(&query_material(
		run,
		frame.task,
		frame.inputs,
		aidash_domain::context::source::MAX_SOURCE_BYTES,
	)?))
}
