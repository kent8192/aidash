//! Workspace permission precedes record-specific disclosure for mapped viewers.
use crate::{Result, ports::graph::GraphVisibility};
use aidash_domain::federation::graph::Candidate;

pub async fn visible(scope: &mut dyn GraphVisibility, candidate: &Candidate) -> Result<bool> {
	let workspace = match candidate {
		Candidate::Workspace(row) => Some(row.id),
		Candidate::Task(row) => Some(row.workspace_id),
		Candidate::Run(row) => Some(row.workspace_id),
		Candidate::Artifact(row) => Some(row.workspace_id),
		Candidate::Conversation(row) => Some(row.workspace_id),
		Candidate::Registry(_) => None,
	};
	if scope.operator() {
		return if let Some(id) = workspace {
			scope.operator_workspace(id).await
		} else {
			Ok(true)
		};
	}
	if let Candidate::Registry(entry) = candidate {
		return scope.registry(entry, "registry.read").await;
	}
	if !scope
		.workspace(
			workspace.expect("non-catalog candidate has a workspace"),
			"workspace.read",
		)
		.await?
	{
		return Ok(false);
	}
	match candidate {
		Candidate::Workspace(_) => Ok(true),
		Candidate::Conversation(row) => scope.conversation(row, "conversation.read").await,
		_ => scope.record(candidate).await,
	}
}

#[cfg(test)]
mod tests;
