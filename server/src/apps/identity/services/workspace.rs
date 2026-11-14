//! Transactional workspace access for authenticated subjects. Workspace read
//! permission opens the workspace; each contained resource and event requires
//! its own read permission. Run contents retain their source read requirements.
use super::{access::Access, identity::SubjectIdentity};
use crate::{
	Result, apps::execution::serializers::state::StateResponse, config::DeploymentIdentity,
	domain::*, store::Store,
};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone)]
pub struct Workspaces {
	pub store: Store,
	pub identity: SubjectIdentity,
}

impl Access {
	/// Evaluate both stream actions from one locked ownership read. Each call
	/// still uses this transaction's current credential and policy snapshot.
	pub(crate) async fn event_workspaces(&mut self, selected: Option<Uuid>) -> Result<Vec<Uuid>> {
		aidash_application::authorization::workspaces::event_workspaces(
			&mut crate::bootstrap::run_visibility_scope(self),
			selected,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn create_workspace(
		&mut self,
		store: &Store,
		title: &str,
		goal: &str,
		node: Option<&aidash_domain::nodes::NodeName>,
		scratch_space_id: Option<Uuid>,
		policy: &crate::config::ScratchOperatorPolicy,
	) -> Result<Workspace> {
		let mut scope = crate::bootstrap::workspace_mutations(self, store);
		scope.scratch_policy = Some(policy);
		scope.requested_scratch = scratch_space_id;
		aidash_application::workspaces::mutations::create(
			&mut scope,
			Uuid::new_v4(),
			title,
			goal,
			node,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn allowed(&mut self, id: Uuid, action: &str) -> Result<bool> {
		aidash_application::authorization::workspaces::allowed(
			&mut crate::bootstrap::run_visibility_scope(self),
			id,
			action,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn require_workspace(&mut self, id: Uuid, action: &str) -> Result<()> {
		if super::access::subnode_workspace_write(action) {
			self.ensure_subnode_writable(id).await?;
		}
		aidash_application::authorization::workspaces::require(
			&mut crate::bootstrap::run_visibility_scope(self),
			id,
			action,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn visible(&mut self, action: &str) -> Result<Vec<Uuid>> {
		aidash_application::authorization::workspaces::visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			action,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn run_visible(
		&mut self,
		run: impl Into<crate::domain::RunMetadata>,
	) -> crate::Result<bool> {
		let run = run.into();
		aidash_application::authorization::visibility::run_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			&run,
		)
		.await
		.map_err(Into::into)
	}

	/// What the failed [`Self::run_visible`] found in the Run's reads, in terms
	/// this reader may see. `None` when the reader's own policy hides the Run
	/// (`run.read`, `memory.read` or a human request), or when the Run is another Home's.
	pub(crate) async fn run_reads_block(
		&mut self,
		run: impl Into<crate::domain::RunMetadata>,
	) -> crate::Result<Option<aidash_domain::run_state::RunReadsBlock>> {
		let run = run.into();
		let Some(block) = self.read_block.take() else {
			return Ok(None);
		};
		if !run.is_local(&self.node_id) {
			return Ok(None);
		}
		let workspace = self.workspace(run.workspace_id).await?;
		let resource = self.resource("run", run.id, workspace.attributes);
		if !self.decide(&resource, "run.read").await? {
			return Ok(None);
		}
		// The walk may have stopped on a producer Run's reads while checking this Run's Task,
		// before it decided this Run's own memory and human requests. If either hides the Run
		// from this reader, the producer's read is not theirs to hear about, and approving it
		// again would not help.
		let memory = match self.memory_resource(run.clone()).await {
			Ok(memory) => memory,
			Err(crate::Error::Forbidden) => return Ok(None),
			Err(error) => return Err(error),
		};
		if !self.decide(&memory, "memory.read").await?
			|| !self.human_reads(run.workspace_id, run.id).await?
		{
			return Ok(None);
		}
		Ok(Some(read_block_reason(block)))
	}

	/// What stops a person from letting the Run go on: the Run's own read walk alone, without
	/// asking whether this reader may read the Run itself (#763). Declining shows the tool and the
	/// kind of refusal, never content, so it does not depend on `run.read` or on the Run's
	/// Working Area; what the Run read still has to be readable. `None` when nothing stops it,
	/// or when the Run is another Home's.
	pub(crate) async fn run_reads_stop(
		&mut self,
		run: impl Into<crate::domain::RunMetadata>,
	) -> crate::Result<Option<aidash_domain::run_state::RunReadsBlock>> {
		let run = run.into();
		if !run.is_local(&self.node_id) {
			return Ok(None);
		}
		self.read_block = None;
		if self.run_reads_visible(run.id).await? {
			return Ok(None);
		}
		let Some(block) = self.read_block.take() else {
			// The walk refused without saying which read; stay closed.
			return Err(crate::Error::Forbidden);
		};
		Ok(Some(read_block_reason(block)))
	}

	/// Preserve the reason when memory execution rechecks its complete Run graph.
	pub(crate) async fn run_invalidity(
		&mut self,
		run: &crate::domain::RunMetadata,
	) -> crate::Result<Option<aidash_domain::semantic::Failure>> {
		use aidash_domain::semantic::Failure;
		match self.run_visible(run.clone()).await {
			Ok(true) => return Ok(None),
			Ok(false) | Err(crate::Error::Conflict(_)) => {}
			Err(crate::Error::Forbidden) => return Ok(Some(Failure::Authority)),
			Err(crate::Error::RemoteSemantic(reason)) => return Ok(Some(reason)),
			Err(error) => return Err(error),
		}
		let mut pending = Vec::new();
		let base = aidash_application::authorization::visibility::classified_base_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			run,
			&mut pending,
		)
		.await
		.map_err(crate::Error::from);
		match base {
			Ok(true) => {}
			Ok(false) | Err(crate::Error::Forbidden) => return Ok(Some(Failure::Authority)),
			Err(crate::Error::Conflict(_)) => return Ok(Some(Failure::Configuration)),
			Err(crate::Error::RemoteSemantic(reason)) => return Ok(Some(reason)),
			Err(error) => return Err(error),
		}
		let reason =
			aidash_application::authorization::visibility::provenance::run_reads_invalidity_with_pending(
				&mut crate::bootstrap::run_visibility_scope(self),
				run.id,
				pending,
			)
			.await
			.map_err(crate::Error::from);
		match reason {
			Ok(reason) => Ok(reason.or(Some(Failure::Invalidated))),
			Err(crate::Error::Forbidden) => Ok(Some(Failure::Authority)),
			Err(crate::Error::RemoteSemantic(reason)) => Ok(Some(reason)),
			Err(error) => Err(error),
		}
	}

	pub(crate) async fn run_base_visible(
		&mut self,
		run: impl Into<crate::domain::RunMetadata>,
	) -> crate::Result<bool> {
		let run = run.into();
		aidash_application::authorization::visibility::base_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			&run,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn event_visible(&mut self, event: &Event) -> crate::Result<bool> {
		aidash_application::authorization::visibility::event_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			event,
		)
		.await
		.map_err(Into::into)
	}

	/// Event disclosure within one scan; `runs` holds the scan's Run verdicts.
	pub(crate) async fn event_visible_in(
		&mut self,
		event: &Event,
		runs: &mut aidash_application::ports::authorization::visibility::RunVerdicts,
	) -> crate::Result<bool> {
		aidash_application::authorization::visibility::event_visible_in(
			&mut crate::bootstrap::run_visibility_scope(self),
			event,
			runs,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn workspace_snapshot(&mut self, id: Uuid) -> Result<WorkspaceSnapshot> {
		aidash_application::authorization::projection::snapshot(
			&mut crate::bootstrap::run_visibility_scope(self),
			id,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn workspace_observation(
		&mut self,
		id: Uuid,
		offset: usize,
		limit: usize,
		exclude_run: Option<Uuid>,
	) -> Result<Value> {
		aidash_application::authorization::projection::observation(
			&mut crate::bootstrap::run_visibility_scope(self),
			id,
			offset,
			limit,
			exclude_run,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn workspace_observation_fitted<F>(
		&mut self,
		id: Uuid,
		offset: usize,
		limit: usize,
		mut fits: F,
	) -> Result<Option<(usize, Value)>>
	where
		F: FnMut(usize, &Value) -> Result<bool>,
	{
		aidash_application::authorization::projection::observation_fitted(
			&mut crate::bootstrap::run_visibility_scope(self),
			id,
			offset,
			limit,
			|limit, output| fits(limit, output).map_err(Into::into),
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn workspace_record(
		&mut self,
		workspace_id: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<Value> {
		aidash_application::authorization::records::record(
			&mut crate::bootstrap::run_visibility_scope(self),
			workspace_id,
			kind,
			id,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn workspace_children(
		&mut self,
		workspace_id: Uuid,
		parent_id: Uuid,
	) -> Result<ChildTaskSummary> {
		aidash_application::authorization::records::children(
			&mut crate::bootstrap::run_visibility_scope(self),
			workspace_id,
			parent_id,
		)
		.await
		.map_err(Into::into)
	}
}

impl Workspaces {
	pub async fn child_task_summary(
		&self,
		workspace: Uuid,
		parent: Uuid,
	) -> Result<ChildTaskSummary> {
		let mut access = Access::begin(&self.store, &self.identity).await?;
		let result = access.workspace_children(workspace, parent).await;
		access.finish(result).await
	}

	pub async fn task_page(
		&self,
		offset: u64,
	) -> Result<crate::apps::workspaces::serializers::tasks::TaskPage> {
		let mut access = Access::begin(&self.store, &self.identity).await?;
		let result = aidash_application::authorization::state::task_page(
			&mut crate::bootstrap::run_visibility_scope(&mut access),
			offset,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}

	pub async fn create(
		&self,
		title: &str,
		goal: &str,
		node: Option<&aidash_domain::nodes::NodeName>,
		scratch_space_id: Option<Uuid>,
		policy: &crate::config::ScratchOperatorPolicy,
	) -> Result<Workspace> {
		let mut access = Access::begin(&self.store, &self.identity).await?;
		let result = access
			.create_workspace(&self.store, title, goal, node, scratch_space_id, policy)
			.await;
		access.finish(result).await
	}

	pub async fn update(&self, id: Uuid, revision: i64, state: Value) -> Result<Workspace> {
		let mut access = Access::begin(&self.store, &self.identity).await?;
		let result = aidash_application::workspaces::mutations::update(
			&mut crate::bootstrap::workspace_mutations(&mut access, &self.store),
			id,
			revision,
			state,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}

	pub async fn create_task(&self, id: Uuid, input: &NewTask, key: Option<&str>) -> Result<Task> {
		let mut access = Access::begin(&self.store, &self.identity).await?;
		let result = aidash_application::workspaces::mutations::create_task(
			&mut crate::bootstrap::workspace_mutations(&mut access, &self.store),
			id,
			input,
			key,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}

	pub async fn message(&self, id: Uuid, content: &str) -> Result<()> {
		self.message_keyed(id, content, None).await
	}

	pub async fn message_keyed(&self, id: Uuid, content: &str, key: Option<Uuid>) -> Result<()> {
		let nonce = key.unwrap_or_else(Uuid::new_v4);
		let mut access = Access::begin(&self.store, &self.identity).await?;
		let result = aidash_application::workspaces::mutations::message(
			&mut crate::bootstrap::workspace_mutations(&mut access, &self.store),
			id,
			content,
			nonce,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}

	pub async fn snapshot(&self, id: Uuid) -> Result<WorkspaceSnapshot> {
		let mut access = Access::begin(&self.store, &self.identity).await?;
		let result = access.workspace_snapshot(id).await;
		access.finish(result).await
	}

	pub async fn state(&self, deployment: DeploymentIdentity) -> Result<StateResponse> {
		let mut access = Access::begin(&self.store, &self.identity).await?;
		let result = aidash_application::authorization::state::state(
			&mut crate::bootstrap::run_visibility_scope(&mut access),
		)
		.await
		.map(|state| StateResponse {
			offering: crate::bootstrap::build::offering().as_str().to_owned(),
			access: crate::api_schema::AccessProfile::Subject {
				tenant: self.identity.tenant.clone(),
				subject: self.identity.subject.clone(),
			},
			deployment,
			registry: state.registry,
			workspaces: state.workspaces,
			tasks: state.tasks,
			artifacts: state.artifacts,
			runs: state.runs,
			human_requests: state.human_requests,
			conversations: state.conversations,
			events: state.events,
		})
		.map_err(Into::into);
		access.finish(result).await
	}

	pub async fn events(
		&self,
		after: i64,
		workspace: Option<Uuid>,
		limit: i64,
	) -> Result<Vec<Event>> {
		self.read_events(after, workspace, limit, true)
			.await
			.map(|(events, _)| events)
	}

	pub(crate) async fn events_with_cursor(
		&self,
		after: i64,
		workspace: Option<Uuid>,
		limit: i64,
	) -> Result<(Vec<Event>, i64)> {
		self.read_events(after, workspace, limit, true).await
	}

	/// Polling itself does not append decision audits. Every delivered frame is
	/// separately checked and audited by can_emit, including buffered frames.
	pub async fn poll_events(
		&self,
		after: i64,
		workspace: Option<Uuid>,
		limit: i64,
	) -> Result<(Vec<Event>, i64)> {
		self.read_events(after, workspace, limit, false).await
	}

	/// Idle checks use one unlocked snapshot; each emitted frame still obtains a live lease.
	pub(crate) async fn stream_authority(&self, workspace: Option<Uuid>) -> Result<()> {
		aidash_application::authorization::stream::authority(
			&crate::bootstrap::stream_authority_store(self),
			workspace,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn stream_page(
		&self,
		after: i64,
		workspace: Option<Uuid>,
		on_query: impl FnOnce(),
	) -> Result<(Vec<Event>, i64, bool)> {
		let mut access = Access::begin(&self.store, &self.identity).await?;
		access.audit = false;
		let result = aidash_application::authorization::stream::stream_page(
			&mut crate::bootstrap::run_visibility_scope(&mut access),
			after,
			workspace,
			on_query,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}

	async fn read_events(
		&self,
		after: i64,
		workspace: Option<Uuid>,
		limit: i64,
		audit: bool,
	) -> Result<(Vec<Event>, i64)> {
		let mut access = Access::begin(&self.store, &self.identity).await?;
		access.audit = audit;
		let result = aidash_application::authorization::stream::read_events(
			&mut crate::bootstrap::run_visibility_scope(&mut access),
			after,
			workspace,
			limit,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}

	/// [`Self::can_emit`] for a caller that answers one person's request: a
	/// refusal reached while a Node did not answer is that Node's silence, so
	/// it is `NodeUnreachable` and not `false` (#751). The event stream keeps
	/// [`Self::can_emit`], which drops the frame.
	pub async fn can_emit_naming_silence(&self, event: &Event) -> Result<bool> {
		let mut access = Access::begin(&self.store, &self.identity).await?;
		let result = aidash_application::authorization::stream::can_emit(
			&mut crate::bootstrap::run_visibility_scope(&mut access),
			event,
		)
		.await
		.map_err(Into::into);
		access.finish_deciding(result).await
	}

	pub async fn can_emit(&self, event: &Event) -> Result<bool> {
		let mut access = Access::begin(&self.store, &self.identity).await?;
		let result = aidash_application::authorization::stream::can_emit(
			&mut crate::bootstrap::run_visibility_scope(&mut access),
			event,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}
}

/// The reason a person is given for a Run's read walk stopping, in terms that name no content.
fn read_block_reason(
	block: aidash_application::authorization::visibility::provenance::ReadBlock,
) -> aidash_domain::run_state::RunReadsBlock {
	use aidash_application::authorization::visibility::provenance::ReadBlock;
	use aidash_domain::run_state::RunReadsBlock;
	match block {
		ReadBlock::Definition(_) => RunReadsBlock::DefinitionNotReadable,
		ReadBlock::RemoteRead => RunReadsBlock::RemoteRead,
		ReadBlock::SemanticRead => RunReadsBlock::SemanticRead,
		ReadBlock::Source(source) => RunReadsBlock::Source { source },
	}
}
