//! Authorized state pages share one live policy snapshot and transaction.
use crate::Result;
use aidash_domain::{
	Artifact, Conversation, Event, HumanRequest, RunInspection, RunMetadata, Task, Workspace,
	policy::Resource, registry::Entry, run_state::RawRun, workspaces::TaskPage,
};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait WorkspaceListingScope: Send {
	async fn visible(&mut self, action: &str) -> Result<Vec<Uuid>>;
	async fn task_page(&mut self, workspaces: &[Uuid], offset: u64) -> Result<TaskPage>;
}
#[async_trait]
pub trait WorkspaceStateScope: Send {
	async fn allowed(&mut self, workspace: Uuid, action: &str) -> Result<bool>;
	async fn registry(&mut self) -> Result<Vec<Entry>>;
	async fn workspace_rows(&mut self, workspaces: &[Uuid]) -> Result<Vec<Workspace>>;
	/// Candidate pages contain at most PAGE_SIZE rows, ordered newest first.
	async fn workspace_candidates(
		&mut self,
		node: Option<&str>,
		cursor: Option<Uuid>,
	) -> Result<Vec<Uuid>>;
	async fn task_candidates(&mut self, node: Option<&str>) -> Result<Vec<Task>>;
	async fn artifact_candidates(&mut self, node: Option<&str>) -> Result<Vec<Artifact>>;
	async fn run_candidates(&mut self, node: Option<&str>) -> Result<Vec<RawRun>>;
	async fn pending_human_candidates(&mut self, node: Option<&str>) -> Result<Vec<HumanRequest>>;
	async fn conversation_candidates(&mut self, node: Option<&str>) -> Result<Vec<Conversation>>;
	async fn latest_visible_events(&mut self, node: Option<&str>) -> Result<Vec<Event>>;
	async fn task_visible(&mut self, task: &Task) -> Result<bool>;
	async fn artifact_visible(&mut self, artifact: &Artifact) -> Result<bool>;
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool>;
	async fn visible_run(&mut self, id: Uuid) -> Result<Option<RunInspection>>;
	async fn human_resource(&mut self, request: &HumanRequest) -> Result<Resource>;
	async fn conversation_resource(&mut self, conversation: &Conversation) -> Result<Resource>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
}
