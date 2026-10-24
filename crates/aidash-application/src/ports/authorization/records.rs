//! Single records and child summaries keep the caller's authority and read journal.
use crate::Result;
use aidash_domain::{
	Artifact, Event, Message, Task, TaskStatus, Workspace, WorkspaceSnapshot, policy::Resource,
};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait WorkspaceRecordScope: Send {
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	async fn workspace_row(&mut self, workspace_id: Uuid) -> Result<Workspace>;
	async fn task_record(&mut self, workspace_id: Uuid, id: Uuid) -> Result<Option<Task>>;
	async fn artifact_record(&mut self, workspace_id: Uuid, id: Uuid) -> Result<Option<Artifact>>;
	async fn message_record(&mut self, workspace_id: Uuid, id: Uuid) -> Result<Option<Message>>;
	async fn event_record(&mut self, workspace_id: Uuid, id: Uuid) -> Result<Option<Event>>;
	async fn task_visible(&mut self, row: &Task) -> Result<bool>;
	async fn artifact_visible(&mut self, row: &Artifact) -> Result<bool>;
	async fn message_visible(&mut self, row: &Message) -> Result<bool>;
	async fn event_visible(&mut self, row: &Event) -> Result<bool>;
	async fn track(&mut self, snapshot: &WorkspaceSnapshot) -> Result<()>;
}
pub struct ChildTaskRecord {
	pub id: Uuid,
	pub created_by: String,
	pub status: TaskStatus,
}
#[async_trait]
pub trait ChildSummaryScope: Send {
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn children(
		&mut self,
		workspace_id: Uuid,
		parent_id: Uuid,
		after: Option<Uuid>,
	) -> Result<Vec<ChildTaskRecord>>;
	async fn visible(
		&mut self,
		workspace: &Resource,
		workspace_id: Uuid,
		id: Uuid,
		created_by: &str,
	) -> Result<bool>;
	async fn track_tasks(&mut self, workspace: Uuid, tasks: &[Uuid]) -> Result<()>;
}
