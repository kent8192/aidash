//! Read authorization over native records, retaining the complete dependency graph.
use super::NativeAccess;
use crate::apps::identity::models::DecisionOwner;
use crate::authorization::policy::Resource;
use crate::{Result, domain};
use uuid::Uuid;

impl NativeAccess {
	pub(crate) async fn semantic_reads_invalidity(
		&mut self,
		run: Uuid,
		pending: &mut Vec<Uuid>,
	) -> Result<Option<aidash_domain::semantic::Failure>> {
		aidash_application::semantic::memory::reads_invalidity(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			run,
			pending,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn run_for_interaction(&mut self, id: Uuid) -> Result<domain::Run> {
		let mark = self.access.call_mark();
		let result = aidash_application::authorization::workspaces::run_for_interaction(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			id,
		)
		.await
		.map_err(Into::into);
		result.map_err(|error| self.access.named_since(mark, error))
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

	pub(crate) async fn run_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		let key = (
			self.access.node_id.clone(),
			run,
			self.access.authority_context(),
		);
		// This walk does not verify remote dependencies as `Access` does, so
		// its verdicts are kept apart from those (#404).
		let verdict_key = (key.0.clone(), run, format!("native:{}", key.2));
		if let Some(verdict) = self.access.read_verdicts.get(&verdict_key) {
			return Ok(verdict);
		}
		let outermost = self.access.checking_reads.is_idle();
		let Some(_visit) = self.access.checking_reads.enter(key) else {
			return Ok(true);
		};
		let walk = self
			.access
			.begin_read_walk(DecisionOwner::Run(run), outermost);
		let result = self.run_reads_visible_in(run).await;
		// A `false` reached while a Node did not answer is not a verdict (#751).
		let unconfirmed = matches!(result, Ok(false)) && !self.access.unavailable_peers.is_empty();
		match result {
			Ok(_) if unconfirmed && outermost => self.access.read_verdicts.abandon(),
			Ok(_) if unconfirmed => {}
			Ok(verdict) => self
				.access
				.read_verdicts
				.record(verdict_key, verdict, outermost),
			Err(_) if outermost => self.access.read_verdicts.abandon(),
			Err(_) => {}
		}
		self.access.end_read_walk(walk, result).await
	}

	async fn run_reads_visible_in(&mut self, run: Uuid) -> Result<bool> {
		aidash_application::authorization::visibility::provenance::run_reads_visible(
			&mut crate::bootstrap::native_run_visibility_scope(self),
			run,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn remote_reads(
		&mut self,
		run: Uuid,
	) -> Result<aidash_application::ports::federation::registry_reads::RemoteReads> {
		let transport = crate::bootstrap::native_registry_transport(self);
		aidash_application::federation::registry_reads::reads(
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
		// A refusal reached while a Node did not answer is not a verdict (#770).
		if !allowed && !self.access.unavailable_peers.is_empty() {
			return;
		}
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
