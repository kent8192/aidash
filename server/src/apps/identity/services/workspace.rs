//! Transactional workspace access for authenticated subjects. Workspace read
//! permission opens the workspace; each contained resource and event requires
//! its own read permission. Run contents retain their source read requirements.
use super::{access::Access, identity::SubjectIdentity};
use crate::{
	Result, apps::execution::serializers::state::StateResponse, config::NodeIdentity, domain::*,
	store::Store,
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
	) -> Result<Workspace> {
		aidash_application::workspaces::mutations::create(
			&mut crate::bootstrap::workspace_mutations(self, store),
			Uuid::new_v4(),
			title,
			goal,
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
	) -> Result<Value> {
		aidash_application::authorization::projection::observation(
			&mut crate::bootstrap::run_visibility_scope(self),
			id,
			offset,
			limit,
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

	pub async fn create(&self, title: &str, goal: &str) -> Result<Workspace> {
		let mut access = Access::begin(&self.store, &self.identity).await?;
		let result = access.create_workspace(&self.store, title, goal).await;
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

	pub async fn state(&self, node: NodeIdentity) -> Result<StateResponse> {
		let mut access = Access::begin(&self.store, &self.identity).await?;
		let result = aidash_application::authorization::state::state(
			&mut crate::bootstrap::run_visibility_scope(&mut access),
		)
		.await
		.map(|state| StateResponse {
			access: crate::api_schema::AccessProfile::Subject {
				tenant: self.identity.tenant.clone(),
				subject: self.identity.subject.clone(),
			},
			node,
			registry: state.registry,
			workspaces: state.workspaces,
			tasks: state.tasks,
			artifacts: state.artifacts,
			runs: state.runs,
			human_requests: state.human_requests,
			conversations: state.conversations,
			events: state.events,
			peers: vec![],
			installations: vec![],
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
