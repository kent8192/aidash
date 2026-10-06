//! Authorized state pages share one live policy snapshot and transaction.
use crate::Result;
use aidash_domain::{
	Artifact, Conversation, Event, HumanRequest, RunMetadata, Workspace, policy::Resource,
	registry::Entry, run_state::RawRun, workspaces::TaskPage,
};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait WorkspaceListingScope: Send {
	async fn visible(&mut self, action: &str) -> Result<Vec<Uuid>>;
	async fn task_page(&mut self, workspaces: &[Uuid], offset: u64) -> Result<TaskPage>;
}
#[async_trait]
pub trait WorkspaceStateScope: WorkspaceListingScope {
	async fn allowed(&mut self, workspace: Uuid, action: &str) -> Result<bool>;
	async fn registry(&mut self) -> Result<Vec<Entry>>;
	async fn workspace_rows(&mut self, workspaces: &[Uuid]) -> Result<Vec<Workspace>>;
	async fn latest_visible_events(
		&mut self,
		workspaces: &[Uuid],
		include_marketplace: bool,
	) -> Result<Vec<Event>>;
	async fn artifact_rows(&mut self, workspaces: &[Uuid], offset: u64) -> Result<Vec<Artifact>>;
	/// Return unvalidated storage state; current visibility precedes per-row inspection.
	async fn raw_runs(&mut self, workspaces: &[Uuid], offset: i64) -> Result<Vec<RawRun>>;
	async fn human_rows(&mut self, runs: &[Uuid], offset: i64) -> Result<Vec<HumanRequest>>;
	async fn conversation_rows(
		&mut self,
		workspaces: &[Uuid],
		offset: i64,
	) -> Result<Vec<Conversation>>;
	async fn artifact_visible(&mut self, artifact: &Artifact) -> Result<bool>;
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool>;
	async fn human_resource(&mut self, request: &HumanRequest) -> Result<Resource>;
	async fn conversation_resource(&mut self, conversation: &Conversation) -> Result<Resource>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
}
