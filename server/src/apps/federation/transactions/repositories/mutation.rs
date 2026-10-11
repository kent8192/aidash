//! Native mutation primitives borrow the participant's original transaction.
use crate::apps::{
	execution::{
		models::{Run, event_records},
		repositories::inference,
	},
	federation::remote::models::Delegation,
	identity::models::{AuthorizationExecution, AuthorizationRunOutput},
	registry::repositories::NativeScope,
	workspaces::models::{Task as TaskRecord, Workspace},
};
use aidash_application::{
	Result,
	ports::{
		registry::{DefinitionLookup, DefinitionWriter},
		transactions::mutation::MutationScope,
	},
};
use aidash_domain::{
	Artifact, ArtifactInput, Task, Workspace as WorkspaceContract,
	provider::progress::ProgressOutcome, registry::Entry, run_state::RawRun,
};
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use serde_json::Value;
use uuid::Uuid;

/// Borrows the participant transaction. Interruptions written here are recorded
/// by the owner only after that transaction commits.
pub(crate) struct Scope<'a>(
	pub(crate) &'a mut dyn TransactionExecutor,
	pub(crate) Vec<ProgressOutcome>,
);

#[async_trait]
impl DefinitionLookup for Scope<'_> {
	async fn definition(&mut self, id: &str, version: &str) -> Result<Entry> {
		NativeScope(self.0).definition(id, version).await
	}
	async fn overrides(&mut self, id: &str, version: &str) -> Result<Option<Value>> {
		NativeScope(self.0).overrides(id, version).await
	}
	async fn executor_kind(&mut self, id: &str, version: &str) -> Result<Option<String>> {
		NativeScope(self.0).executor_kind(id, version).await
	}
}

#[async_trait]
impl DefinitionWriter for Scope<'_> {
	async fn insert_definition(&mut self, entry: &Entry) -> Result<bool> {
		NativeScope(self.0).insert_definition(entry).await
	}
}

#[async_trait]
impl MutationScope for Scope<'_> {
	async fn replace_workspace(
		&mut self,
		id: Uuid,
		revision: i64,
		state: Value,
	) -> Result<WorkspaceContract> {
		Workspace::replace_state(self.0, id, revision, state)
			.await
			.map_err(Into::into)
	}
	async fn lock_task(&mut self, id: Uuid) -> Result<Task> {
		TaskRecord::lock(self.0, id)
			.await
			.map(Into::into)
			.map_err(Into::into)
	}
	async fn unfinished_children(&mut self, id: Uuid) -> Result<bool> {
		TaskRecord::unfinished_children(self.0, id)
			.await
			.map_err(Into::into)
	}
	async fn delegated_node(&mut self, task: Uuid) -> Result<Option<String>> {
		Delegation::node_for_task(self.0, task)
			.await
			.map_err(Into::into)
	}
	async fn complete_task(
		&mut self,
		task: &Task,
		artifact: &ArtifactInput,
		key: &str,
	) -> Result<(Task, Artifact)> {
		TaskRecord::complete(self.0, task, artifact, key)
			.await
			.map_err(Into::into)
	}
	async fn source_run(&mut self, task: Uuid) -> Result<Option<Uuid>> {
		AuthorizationExecution::source_run_for_task(self.0, task)
			.await
			.map_err(Into::into)
	}
	async fn record_output(
		&mut self,
		run: Uuid,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<()> {
		AuthorizationRunOutput::record(self.0, run, workspace, kind, id)
			.await
			.map_err(Into::into)
	}
	async fn lock_run(&mut self, id: Uuid) -> Result<(RawRun, bool)> {
		let (record, leased) = Run::lock_with_lease(self.0, id).await?;
		// RawRun preserves JSON context and pending without decoding execution state.
		Ok((
			serde_json::from_value(serde_json::to_value(record)?)?,
			leased,
		))
	}
	async fn complete_run(&mut self, id: Uuid) -> Result<()> {
		Run::complete(self.0, id).await?;
		// Completion is validated as Home-local, so the Home node records markers.
		let run = inference::target(&mut *self.0, id).await?;
		let closed = inference::close_pending(self.0, &run.home_node, id, None).await?;
		self.1.extend(closed);
		Ok(())
	}
	async fn append_event(
		&mut self,
		node: &str,
		workspace: Option<Uuid>,
		kind: &str,
		data: Value,
	) -> Result<()> {
		event_records::append(self.0, node, workspace, kind, data)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}
