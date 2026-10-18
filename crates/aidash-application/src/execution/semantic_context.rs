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
	if repository.remote() {
		repository.suspend().await?;
		let result = repository
			.remote_context(task, inputs, &query, budget)
			.await?;
		repository.refresh_remote().await?;
		return Ok(result);
	}
	let mut scope = repository.local_scope().await?;
	let result = match scope.retrieve(&query, budget).await? {
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
