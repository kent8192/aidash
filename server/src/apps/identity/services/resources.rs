//! Resource attributes come from stored rows and their authoritative workspace.
use super::{access::Access, policy::Resource};
use crate::{
	Result,
	domain::{Artifact, Event, Message, Task},
};
use uuid::Uuid;

impl Access {
	pub(crate) async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		aidash_application::authorization::visibility::resources::task_resource(
			&mut crate::bootstrap::run_visibility_scope(self),
			task,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn task_read(&mut self, id: Uuid) -> Result<Task> {
		aidash_application::authorization::visibility::resources::task_read(
			&mut crate::bootstrap::run_visibility_scope(self),
			id,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn task_visible(&mut self, task: &Task) -> Result<bool> {
		aidash_application::authorization::visibility::resources::task_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			task,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn task_summary_visible(
		&mut self,
		workspace: &Resource,
		workspace_id: Uuid,
		task_id: Uuid,
		created_by: &str,
	) -> Result<bool> {
		aidash_application::authorization::visibility::resources::task_summary_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			workspace,
			workspace_id,
			task_id,
			created_by,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn related_tasks(
		&mut self,
		workspace: Uuid,
		input: &crate::domain::NewTask,
	) -> Result<()> {
		aidash_application::authorization::visibility::resources::related_tasks(
			&mut crate::bootstrap::run_visibility_scope(self),
			workspace,
			input,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn artifact_visible(&mut self, artifact: &Artifact) -> Result<bool> {
		aidash_application::authorization::visibility::resources::artifact_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			artifact,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn output_visible(
		&mut self,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<bool> {
		aidash_application::authorization::visibility::provenance::output_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			workspace,
			kind,
			id,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn memory_resource(
		&mut self,
		run: impl Into<crate::domain::RunMetadata>,
	) -> Result<Resource> {
		let run = run.into();
		let workspace = self.workspace(run.workspace_id).await?;
		Ok(self.resource(
			"memory",
			&run.agent_id,
			aidash_application::authorization::visibility::memory_attributes(
				&run,
				workspace.attributes,
			),
		))
	}
	pub(crate) async fn artifact_creation_resource(
		&mut self,
		task: Uuid,
		creator: &str,
	) -> Result<Resource> {
		aidash_application::authorization::visibility::resources::artifact_creation_resource(
			&mut crate::bootstrap::run_visibility_scope(self),
			task,
			creator,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn message_visible(&mut self, message: &Message) -> Result<bool> {
		aidash_application::authorization::visibility::resources::message_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			message,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn resource_event_visible(&mut self, event: &Event) -> Result<Option<bool>> {
		aidash_application::authorization::visibility::resources::event_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			event,
		)
		.await
		.map_err(Into::into)
	}
}

impl Access {
	pub(crate) async fn track_snapshot(
		&mut self,
		snapshot: &crate::domain::WorkspaceSnapshot,
	) -> Result<()> {
		aidash_application::authorization::journals::track_snapshot(
			&mut crate::bootstrap::run_visibility_scope(self),
			snapshot,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn track_registry(
		&mut self,
		entries: &[crate::registry::Entry],
	) -> Result<()> {
		aidash_application::authorization::journals::track_registry(
			&mut crate::bootstrap::run_visibility_scope(self),
			entries,
		)
		.await
		.map_err(Into::into)
	}

	/// Walk recorded run dependencies iteratively; cycles between observation
	/// journals must terminate without skipping any resource's current policy.
	pub(crate) async fn run_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		let key = (self.node_id.clone(), run, self.authority_context());
		let Some(_visit) = self.checking_reads.enter(key) else {
			return Ok(true);
		};
		let coordinator = self.dependency_frontier.is_none();
		if coordinator {
			self.dependency_frontier = Some(vec![]);
		}
		let mut result = self.run_reads_visible_in(run).await;
		if coordinator {
			let pending = self.dependency_frontier.take().unwrap_or_default();
			if matches!(result, Ok(true)) {
				result = self.verify_dependencies(pending).await;
			}
		}
		result
	}
	async fn run_reads_visible_in(&mut self, run: Uuid) -> Result<bool> {
		aidash_application::authorization::visibility::provenance::run_reads_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			run,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn grant_reads_visible(&mut self, grant: Uuid) -> Result<bool> {
		aidash_application::authorization::journals::grant_reads_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			grant,
		)
		.await
		.map_err(Into::into)
	}
}
