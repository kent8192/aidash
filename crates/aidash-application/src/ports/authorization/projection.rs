//! Workspace pages and snapshots borrow one live authorization and database lease.
use crate::Result;
use aidash_domain::{
	Artifact, Event, Message, Task, Workspace, WorkspaceSnapshot, policy::Resource,
};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait WorkspaceProjection: Send {
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	async fn task_visible(&mut self, row: &Task) -> Result<bool>;
	async fn artifact_visible(&mut self, row: &Artifact) -> Result<bool>;
	async fn message_visible(&mut self, row: &Message) -> Result<bool>;
	async fn event_visible(&mut self, row: &Event) -> Result<bool>;
	async fn task_rows(&mut self, workspaces: &[Uuid], cursor: u64) -> Result<Vec<Task>>;
	async fn event_rows(
		&mut self,
		workspaces: &[Uuid],
		include_marketplace: bool,
		cursor: i64,
		page_size: usize,
	) -> Result<Vec<Event>>;
	async fn message_rows(&mut self, workspace: Uuid, offset: u64) -> Result<Vec<Message>>;
	async fn workspace_row(&mut self, id: Uuid) -> Result<Workspace>;
	async fn workspace_tasks(&mut self, id: Uuid) -> Result<Vec<Task>>;
	async fn workspace_artifacts(&mut self, id: Uuid) -> Result<Vec<Artifact>>;
	/// Retain the independent durable read-membership commit before external disclosure.
	async fn track(&mut self, snapshot: &WorkspaceSnapshot) -> Result<()>;
}
