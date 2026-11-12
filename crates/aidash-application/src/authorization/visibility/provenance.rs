//! Iterative journal traversal terminates cycles without omitting any current read policy.
use super::resources::{artifact_visible, conversation_resource, message_visible, task_visible};
use crate::ports::authorization::visibility::resources::ResourceVisibilityScope;
use crate::{
	Error, Result,
	ports::{
		authorization::visibility::provenance::ReadProvenanceScope,
		federation::registry_reads::RemoteReads,
	},
};
use aidash_domain::semantic::Failure;
use aidash_domain::{Artifact, HumanRequest, Task, policy::Resource, registry::EntityRef};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeSet;
use uuid::Uuid;
/// The read at which a walk of a Run's recorded reads stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadBlock {
	Definition(EntityRef),
	RemoteRead,
	SemanticRead,
	Source(String),
}

async fn registry_read_block(
	scope: &mut dyn ReadProvenanceScope,
	run: Uuid,
) -> Result<Option<EntityRef>> {
	for reference in scope.registry_entries(run).await? {
		match scope.catalog_read(run, &reference).await {
			Ok(()) => {}
			Err(Error::Forbidden) => return Ok(Some(reference)),
			Err(error) => return Err(error),
		}
	}
	Ok(None)
}
pub async fn registry_reads_visible(
	scope: &mut dyn ReadProvenanceScope,
	run: Uuid,
) -> Result<bool> {
	Ok(registry_read_block(scope, run).await?.is_none())
}
pub async fn run_reads_visible(scope: &mut dyn ReadProvenanceScope, run: Uuid) -> Result<bool> {
	Ok(run_reads_block(scope, run).await?.is_none())
}
/// Walk recorded reads; a refusal takes precedence over the first silent dependency.
pub async fn run_reads_block(
	scope: &mut dyn ReadProvenanceScope,
	run: Uuid,
) -> Result<Option<ReadBlock>> {
	let mut pending = vec![run];
	let mut visited = BTreeSet::new();
	let mut silent = None;
	while let Some(run) = pending.pop() {
		if !visited.insert(run) || !scope.first_visit(run) {
			continue;
		}
		let opened = scope.open_run_reads(run).await?;
		let block = one_run_reads_block(scope, run, &mut pending).await;
		if opened {
			scope.close_run_reads();
		}
		if let Some(block) = block?
			&& let Some(refused) = hold_or_refuse(scope, &mut silent, block)
		{
			return Ok(Some(refused));
		}
	}
	Ok(restore_silence(scope, silent))
}

fn hold_or_refuse(
	scope: &mut dyn ReadProvenanceScope,
	silent: &mut Option<(String, ReadBlock)>,
	block: ReadBlock,
) -> Option<ReadBlock> {
	match scope.take_row_silence() {
		Some(node) => {
			silent.get_or_insert((node, block));
			None
		}
		None => Some(block),
	}
}

fn restore_silence(
	scope: &mut dyn ReadProvenanceScope,
	silent: Option<(String, ReadBlock)>,
) -> Option<ReadBlock> {
	silent.map(|(node, block)| {
		scope.set_row_silence(Some(node));
		block
	})
}

async fn one_run_reads_block(
	scope: &mut dyn ReadProvenanceScope,
	run: Uuid,
	pending: &mut Vec<Uuid>,
) -> Result<Option<ReadBlock>> {
	let mut silent = None;
	if let Some(reference) = registry_read_block(scope, run).await? {
		return Ok(Some(ReadBlock::Definition(reference)));
	}
	match scope.remote_reads(run).await? {
		RemoteReads::Visible => {}
		RemoteReads::Definition(reference) => return Ok(Some(ReadBlock::Definition(reference))),
		RemoteReads::Unconfirmed => {
			if let Some(block) = hold_or_refuse(scope, &mut silent, ReadBlock::RemoteRead) {
				return Ok(Some(block));
			}
		}
	}
	if !scope.semantic_reads(run).await?
		&& let Some(block) = hold_or_refuse(scope, &mut silent, ReadBlock::SemanticRead)
	{
		return Ok(Some(block));
	}
	if !scope.received_semantic(run).await?
		&& let Some(block) = hold_or_refuse(scope, &mut silent, ReadBlock::SemanticRead)
	{
		return Ok(Some(block));
	}
	let sources = scope.sources(run).await?;
	scope.prepare_sources(&sources).await?;
	for (workspace, kind, id) in sources {
		if !source_visible(scope, workspace, &kind, id, pending).await?
			&& let Some(block) = hold_or_refuse(scope, &mut silent, ReadBlock::Source(kind))
		{
			return Ok(Some(block));
		}
	}
	Ok(restore_silence(scope, silent))
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
		let opened = scope.open_run_reads(run).await?;
		let invalidity = one_run_reads_invalidity(scope, run, &mut pending).await;
		if opened {
			scope.close_run_reads();
		}
		if let Some(reason) = invalidity? {
			return Ok(Some(reason));
		}
	}
	Ok(None)
}

async fn one_run_reads_invalidity(
	scope: &mut dyn ReadProvenanceScope,
	run: Uuid,
	pending: &mut Vec<Uuid>,
) -> Result<Option<Failure>> {
	if !registry_reads_visible(scope, run).await? {
		return Ok(Some(Failure::Authority));
	}
	// A denied definition of the reader's own Tenant reads as a local one does; other
	// peer visibility carries no reason, so it keeps its existing verdict.
	match scope.remote_reads(run).await? {
		RemoteReads::Visible => {}
		RemoteReads::Definition(_) => return Ok(Some(Failure::Authority)),
		RemoteReads::Unconfirmed => return Ok(Some(Failure::Invalidated)),
	}
	if !scope.received_semantic(run).await? {
		return Ok(Some(Failure::Invalidated));
	}
	if let Some(reason) = scope.semantic_invalidity(run, pending).await? {
		return Ok(Some(reason));
	}
	let sources = scope.sources(run).await?;
	scope.prepare_sources(&sources).await?;
	for (workspace, kind, id) in sources {
		if let Some(reason) = source_invalidity(scope, workspace, &kind, id, pending, true).await? {
			return Ok(Some(reason));
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

impl crate::ports::authorization::visibility::RowOutcome for ClassifiedOutputs<'_> {}
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
	let mut silent = None;
	for producer in scope.producers(workspace, kind, id).await? {
		if !scope.producer_reads_visible(producer).await? {
			match scope.take_row_silence() {
				Some(node) => {
					silent.get_or_insert(node);
				}
				None => return Ok(false),
			}
		}
	}
	let visible = silent.is_none();
	scope.set_row_silence(silent);
	Ok(visible)
}
pub async fn output_visible(
	scope: &mut dyn crate::ports::authorization::visibility::provenance::OutputScope,
	workspace: Uuid,
	kind: &str,
	id: Uuid,
) -> Result<bool> {
	let mut silent = None;
	for grant in scope.remote_grants(workspace, kind, id).await? {
		if !scope.grant_visible(grant).await? {
			match scope.take_row_silence() {
				Some(node) => {
					silent.get_or_insert(node);
				}
				None => return Ok(false),
			}
		}
	}
	if !local_output_visible(scope, workspace, kind, id).await? {
		match scope.take_row_silence() {
			Some(node) => {
				silent.get_or_insert(node);
			}
			None => return Ok(false),
		}
	}
	let visible = silent.is_none();
	scope.set_row_silence(silent);
	Ok(visible)
}

#[cfg(test)]
mod output_tests;
