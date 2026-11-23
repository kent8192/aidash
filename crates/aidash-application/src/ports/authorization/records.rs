//! Single records and child summaries keep the caller's authority and read journal.
use crate::{Error, Result};
use aidash_domain::{
	Artifact, Event, Message, Task, TaskStatus, Thread, ThreadSnapshot, policy::Resource,
};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait ThreadRecordScope: Send {
	async fn thread(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	/// Worker reads can give the model instructions when a record is outside its Thread.
	async fn missing_record(&mut self, _kind: &str, _id: Uuid) -> Result<Error> {
		Ok(Error::Forbidden)
	}
	async fn thread_row(&mut self, thread_id: Uuid) -> Result<Thread>;
	async fn task_record(&mut self, thread_id: Uuid, id: Uuid) -> Result<Option<Task>>;
	async fn artifact_record(&mut self, thread_id: Uuid, id: Uuid) -> Result<Option<Artifact>>;
	async fn message_record(&mut self, thread_id: Uuid, id: Uuid) -> Result<Option<Message>>;
	async fn attachment_record(
		&mut self,
		thread_id: Uuid,
		id: Uuid,
	) -> Result<Option<aidash_domain::media::AttachmentRecord>>;

	async fn event_record(&mut self, thread_id: Uuid, id: Uuid) -> Result<Option<Event>>;
	async fn task_visible(&mut self, row: &Task) -> Result<bool>;
	async fn artifact_visible(&mut self, row: &Artifact) -> Result<bool>;
	async fn message_visible(&mut self, row: &Message) -> Result<bool>;
	/// Attachment metadata for already authorized messages, never bytes.
	async fn message_attachments(
		&mut self,
		thread: Uuid,
		messages: &[Uuid],
	) -> Result<Vec<aidash_domain::media::MessageAttachment>>;
	async fn event_visible(&mut self, row: &Event) -> Result<bool>;
	async fn track(&mut self, snapshot: &ThreadSnapshot) -> Result<()>;
}
pub struct ChildTaskRecord {
	pub revision: i64,
	pub id: Uuid,
	pub title: String,
	pub created_by: String,
	pub status: TaskStatus,
	pub dependencies: Vec<Uuid>,
	/// Somebody has the Task. Read for open and claimed children only.
	pub started: bool,
	/// A Run of the parent created the Task and nothing open sits beneath it.
	/// Read for open and claimed children only.
	pub abandonable: bool,
}
#[async_trait]
pub trait ChildSummaryScope: Send + super::visibility::RowOutcome {
	async fn thread(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn children(
		&mut self,
		thread_id: Uuid,
		parent_id: Uuid,
		after: Option<Uuid>,
	) -> Result<Vec<ChildTaskRecord>>;
	async fn visible(
		&mut self,
		thread: &Resource,
		thread_id: Uuid,
		id: Uuid,
		created_by: &str,
	) -> Result<bool>;
	async fn track_tasks(&mut self, thread: Uuid, tasks: &[Uuid]) -> Result<()>;
}
