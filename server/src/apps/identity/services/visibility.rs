//! Read authorization over native records, retaining the complete dependency graph.
use super::NativeAccess;
use crate::authorization::policy::Resource;
use crate::registry::{EntityRef, Entry};
use crate::{Result, domain};
use uuid::Uuid;

impl NativeAccess {
	pub(crate) async fn run_for_interaction(&mut self, id: Uuid) -> Result<domain::Run> {
		aidash_application::authorization::workspaces::run_for_interaction(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			id,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn human_resource(
		&mut self,
		request: &domain::HumanRequest,
	) -> Result<Resource> {
		aidash_application::authorization::visibility::resources::human_resource(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			request,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		aidash_application::authorization::workspaces::resource(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			id,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn run_visible(
		&mut self,
		run: impl Into<domain::RunMetadata>,
	) -> Result<bool> {
		let run = run.into();
		aidash_application::authorization::visibility::local_run_visible(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			&run,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn run_base_visible(&mut self, run: &domain::RunMetadata) -> Result<bool> {
		aidash_application::authorization::visibility::local_base_visible(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			run,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn human_reads(&mut self, workspace: Uuid, run: Uuid) -> Result<bool> {
		aidash_application::authorization::visibility::resources::human_reads(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			workspace,
			run,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn task_visible(&mut self, task: &domain::Task) -> Result<bool> {
		aidash_application::authorization::visibility::resources::task_visible(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			task,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn artifact_visible(&mut self, artifact: &domain::Artifact) -> Result<bool> {
		aidash_application::authorization::visibility::resources::artifact_visible(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			artifact,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn message_visible(&mut self, message: &domain::Message) -> Result<bool> {
		aidash_application::authorization::visibility::resources::message_visible(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			message,
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
		aidash_application::authorization::visibility::provenance::local_output_visible(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			workspace,
			kind,
			id,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn catalog_entry(&mut self, reference: &EntityRef) -> Result<Entry> {
		aidash_application::authorization::catalog::retained::entry(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			reference,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn run_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		let key = (
			self.access.node_id.clone(),
			run,
			self.access.authority_context(),
		);
		let Some(_visit) = self.access.checking_reads.enter(key) else {
			return Ok(true);
		};
		self.run_reads_visible_in(run).await
	}

	async fn run_reads_visible_in(&mut self, run: Uuid) -> Result<bool> {
		aidash_application::authorization::visibility::provenance::run_reads_visible(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			run,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn remote_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		let transport = crate::bootstrap::native_registry_transport(self);
		aidash_application::federation::registry_reads::reads_visible(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			&transport,
			run,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn semantic_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		aidash_application::semantic::memory::reads_visible(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			run,
		)
		.await
		.map_err(Into::into)
	}
}

impl NativeAccess {
	pub(crate) fn cached_run(&self, run: &domain::RunMetadata) -> Option<bool> {
		self.access
			.cached_runs
			.get(&(run.workspace_id, run.id))
			.copied()
	}
	pub(crate) fn remember_run(&mut self, run: &domain::RunMetadata, allowed: bool) {
		self.access
			.cached_runs
			.insert((run.workspace_id, run.id), allowed);
	}
}

impl NativeAccess {
	pub(crate) fn cached_human(&self, id: Uuid) -> Option<bool> {
		self.access.cached_humans.get(&id).copied()
	}
	pub(crate) fn remember_human(&mut self, id: Uuid, allowed: bool) {
		self.access.cached_humans.insert(id, allowed);
	}
}

impl NativeAccess {
	pub(crate) fn inherited_lease(&self) -> bool {
		self.access.inherited_lease
	}
}
