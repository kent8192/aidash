//! Iterative journal traversal terminates cycles without omitting any current read policy.
use super::resources::{artifact_visible, conversation_resource, message_visible, task_visible};
use crate::ports::authorization::visibility::resources::ResourceVisibilityScope;
use crate::{Error, Result, ports::authorization::visibility::provenance::ReadProvenanceScope};
use aidash_domain::semantic::Failure;
use aidash_domain::{Artifact, HumanRequest, Task, policy::Resource};
use async_trait::async_trait;
use serde_json::Value;
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
	run_reads_invalidity_with_pending(scope, run, vec![]).await
}

/// Include producers discovered while checking the Run's own resource gates.
pub async fn run_reads_invalidity_with_pending(
	scope: &mut dyn ReadProvenanceScope,
	run: Uuid,
	mut pending: Vec<Uuid>,
) -> Result<Option<Failure>> {
	pending.push(run);
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
		if let Some(reason) = scope.semantic_invalidity(run, &mut pending).await? {
			return Ok(Some(reason));
		}
		for (workspace, kind, id) in scope.sources(run).await? {
			if let Some(reason) =
				source_invalidity(scope, workspace, &kind, id, &mut pending, true).await?
			{
				return Ok(Some(reason));
			}
		}
	}
	Ok(None)
}

/// Check the Task's current policy while deferring local producer journals.
pub async fn classified_task_visible(
	scope: &mut dyn ReadProvenanceScope,
	task: &Task,
	pending: &mut Vec<Uuid>,
) -> Result<bool> {
	task_visible(&mut ClassifiedOutputs { scope, pending }, task).await
}

/// Resolve a locked semantic output source using the same policies as direct reads,
/// and defer producer journals to the caller's cycle-safe worklist.
pub async fn classified_semantic_output_source(
	scope: &mut dyn ReadProvenanceScope,
	workspace: Uuid,
	source: &aidash_domain::semantic::Source,
	pending: &mut Vec<Uuid>,
) -> Result<Option<String>> {
	use aidash_domain::semantic::Source;
	match source {
		Source::Artifact { id } => {
			let Some(row) = scope.source_artifact(*id, workspace).await? else {
				return Ok(None);
			};
			if !artifact_visible(&mut ClassifiedOutputs { scope, pending }, &row).await? {
				return Ok(None);
			}
			Ok(Some(serde_json::to_string(&row.content)?))
		}
		Source::Message { id } => {
			let Some(row) = scope.source_message(*id, workspace).await? else {
				return Ok(None);
			};
			if !message_visible(&mut ClassifiedOutputs { scope, pending }, &row).await? {
				return Ok(None);
			}
			Ok(Some(row.content))
		}
		_ => unreachable!("classified semantic output must be an artifact or message"),
	}
}

async fn source_invalidity(
	scope: &mut dyn ReadProvenanceScope,
	workspace: Uuid,
	kind: &str,
	id: Uuid,
	pending: &mut Vec<Uuid>,
	classify_outputs: bool,
) -> Result<Option<Failure>> {
	let visible = match kind {
		"workspace_events" => {
			let resource = scope.workspace(workspace).await?;
			scope.decide(&resource, "workspace.events").await?
		}
		"task" => match scope.source_task(id, workspace).await? {
			Some(task) if classify_outputs => {
				task_visible(&mut ClassifiedOutputs { scope, pending }, &task).await?
			}
			Some(task) => task_visible(scope, &task).await?,
			None => return Ok(Some(Failure::Invalidated)),
		},
		"artifact" => match scope.source_artifact(id, workspace).await? {
			Some(row) if classify_outputs => {
				artifact_visible(&mut ClassifiedOutputs { scope, pending }, &row).await?
			}
			Some(row) => artifact_visible(scope, &row).await?,
			None => return Ok(Some(Failure::Invalidated)),
		},
		"attachment" => match scope.source_attachment_message(id, workspace).await? {
			Some(row) if classify_outputs => {
				message_visible(&mut ClassifiedOutputs { scope, pending }, &row).await?
			}
			Some(row) => message_visible(scope, &row).await?,
			None => return Ok(Some(Failure::Invalidated)),
		},
		"message" => match scope.source_message(id, workspace).await? {
			Some(row) if classify_outputs => {
				message_visible(&mut ClassifiedOutputs { scope, pending }, &row).await?
			}
			Some(row) => message_visible(scope, &row).await?,
			None => return Ok(Some(Failure::Invalidated)),
		},
		"run" => match scope.source_run(id, workspace).await? {
			Some(run) => {
				pending.push(run.id);
				if classify_outputs {
					scope
						.classified_run_dependency_visible(&run, pending)
						.await?
				} else {
					scope.run_dependency_visible(&run).await?
				}
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
	Ok(
		source_invalidity(scope, workspace, kind, id, pending, false)
			.await?
			.is_none(),
	)
}

/// Reuse resource policy checks while deferring local producer journals to the
/// classified worklist, which also handles cycles between outputs and reads.
struct ClassifiedOutputs<'a> {
	scope: &'a mut dyn ReadProvenanceScope,
	pending: &'a mut Vec<Uuid>,
}

#[async_trait]
impl ResourceVisibilityScope for ClassifiedOutputs<'_> {
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.scope.resource(kind, id, attributes)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.scope.workspace(id).await
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.scope.decide(resource, action).await
	}
	async fn artifact_task(&mut self, artifact: &Artifact) -> Result<Option<Task>> {
		self.scope.artifact_task(artifact).await
	}
	async fn output_visible(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<bool> {
		match self.scope.output_producers(workspace, kind, id).await? {
			Some(producers) => {
				self.pending.extend(producers);
				Ok(true)
			}
			None => Ok(false),
		}
	}
	fn cached_human(&self, id: Uuid) -> Option<bool> {
		self.scope.cached_human(id)
	}
	fn remember_human(&mut self, id: Uuid, allowed: bool) {
		self.scope.remember_human(id, allowed);
	}
	async fn humans(&mut self, workspace: Uuid, run: Uuid) -> Result<Vec<HumanRequest>> {
		self.scope.humans(workspace, run).await
	}
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
