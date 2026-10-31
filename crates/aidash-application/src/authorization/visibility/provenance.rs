//! Iterative journal traversal terminates cycles without omitting any current read policy.
use super::resources::{artifact_visible, conversation_resource, message_visible, task_visible};
use crate::{Error, Result, ports::authorization::visibility::provenance::ReadProvenanceScope};
use aidash_domain::semantic::Failure;
use std::collections::BTreeSet;
use uuid::Uuid;
pub async fn registry_reads_visible(
	scope: &mut dyn ReadProvenanceScope,
	run: Uuid,
) -> Result<bool> {
	for reference in scope.registry_entries(run).await? {
		match scope.catalog_read(run, &reference).await {
			Ok(()) => {}
			Err(Error::Forbidden) => return Ok(false),
			Err(error) => return Err(error),
		}
	}
	Ok(true)
}
pub async fn run_reads_visible(scope: &mut dyn ReadProvenanceScope, run: Uuid) -> Result<bool> {
	let mut pending = vec![run];
	let mut visited = BTreeSet::new();
	while let Some(run) = pending.pop() {
		if !visited.insert(run) {
			continue;
		}
		if !registry_reads_visible(scope, run).await?
			|| !scope.remote_reads(run).await?
			|| !scope.semantic_reads(run).await?
			|| !scope.received_semantic(run).await?
		{
			return Ok(false);
		}
		for (workspace, kind, id) in scope.sources(run).await? {
			if !source_visible(scope, workspace, &kind, id, &mut pending).await? {
				return Ok(false);
			}
		}
	}
	Ok(true)
}

/// Classify the same dependency graph for memory execution recovery.
pub async fn run_reads_invalidity(
	scope: &mut dyn ReadProvenanceScope,
	run: Uuid,
) -> Result<Option<Failure>> {
	let mut pending = vec![run];
	let mut visited = BTreeSet::new();
	while let Some(run) = pending.pop() {
		if !visited.insert(run) {
			continue;
		}
		if !registry_reads_visible(scope, run).await? {
			return Ok(Some(Failure::Authority));
		}
		// Peer visibility carries no reason; preserve its existing verdict.
		if !scope.remote_reads(run).await? || !scope.received_semantic(run).await? {
			return Ok(Some(Failure::Invalidated));
		}
		if let Some(reason) = scope.semantic_invalidity(run).await? {
			return Ok(Some(reason));
		}
		for (workspace, kind, id) in scope.sources(run).await? {
			if let Some(reason) =
				source_invalidity(scope, workspace, &kind, id, &mut pending).await?
			{
				return Ok(Some(reason));
			}
		}
	}
	Ok(None)
}

async fn source_invalidity(
	scope: &mut dyn ReadProvenanceScope,
	workspace: Uuid,
	kind: &str,
	id: Uuid,
	pending: &mut Vec<Uuid>,
) -> Result<Option<Failure>> {
	let visible = match kind {
		"workspace_events" => {
			let resource = scope.workspace(workspace).await?;
			scope.decide(&resource, "workspace.events").await?
		}
		"task" => match scope.source_task(id, workspace).await? {
			Some(task) => task_visible(scope, &task).await?,
			None => return Ok(Some(Failure::Invalidated)),
		},
		"artifact" => match scope.source_artifact(id, workspace).await? {
			Some(row) => artifact_visible(scope, &row).await?,
			None => return Ok(Some(Failure::Invalidated)),
		},
		"message" => match scope.source_message(id, workspace).await? {
			Some(row) => message_visible(scope, &row).await?,
			None => return Ok(Some(Failure::Invalidated)),
		},
		"run" => match scope.source_run(id, workspace).await? {
			Some(run) => {
				pending.push(run.id);
				scope.run_dependency_visible(&run).await?
			}
			None => return Ok(Some(Failure::Invalidated)),
		},
		"conversation" => match scope.source_conversation(id, workspace).await? {
			Some(row) => {
				let resource = conversation_resource(scope, &row).await?;
				scope.decide(&resource, "conversation.read").await?
			}
			None => return Ok(Some(Failure::Invalidated)),
		},
		"generation" => match scope.source_generation(id, workspace).await? {
			Some(row) => scope.generation_visible(&row).await?,
			None => return Ok(Some(Failure::Invalidated)),
		},
		_ => false,
	};
	Ok((!visible).then_some(Failure::Authority))
}
pub async fn source_visible(
	scope: &mut dyn ReadProvenanceScope,
	workspace: Uuid,
	kind: &str,
	id: Uuid,
	pending: &mut Vec<Uuid>,
) -> Result<bool> {
	Ok(source_invalidity(scope, workspace, kind, id, pending)
		.await?
		.is_none())
}

#[cfg(test)]
mod tests;

/// The native local journal checks local producers; its foreign reader owns grant provenance.
pub async fn local_output_visible(
	scope: &mut dyn crate::ports::authorization::visibility::provenance::LocalOutputScope,
	workspace: Uuid,
	kind: &str,
	id: Uuid,
) -> Result<bool> {
	for producer in scope.producers(workspace, kind, id).await? {
		if !scope.producer_reads_visible(producer).await? {
			return Ok(false);
		}
	}
	Ok(true)
}
pub async fn output_visible(
	scope: &mut dyn crate::ports::authorization::visibility::provenance::OutputScope,
	workspace: Uuid,
	kind: &str,
	id: Uuid,
) -> Result<bool> {
	for grant in scope.remote_grants(workspace, kind, id).await? {
		if !scope.grant_visible(grant).await? {
			return Ok(false);
		}
	}
	local_output_visible(scope, workspace, kind, id).await
}

#[cfg(test)]
mod output_tests;
