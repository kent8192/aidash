//! Thread pages and snapshots borrow one live authorization and database lease.
use super::visibility::{RowOutcome, RunVerdicts};
use crate::Result;
use aidash_domain::{Event, Message, Task, Thread, ThreadSnapshot, policy::Resource};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait ThreadProjection: Send + RowOutcome {
	async fn thread(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	async fn task_visible(&mut self, row: &Task) -> Result<bool>;

	async fn message_visible(&mut self, row: &Message) -> Result<bool>;
	/// Attachment metadata for already authorized messages, never bytes.
	async fn message_attachments(
		&mut self,
		thread: Uuid,
		messages: &[Uuid],
	) -> Result<Vec<aidash_domain::media::MessageAttachment>>;
	/// Event disclosure within one scan; `runs` holds the scan's Run verdicts.
	async fn event_visible(&mut self, row: &Event, runs: &mut RunVerdicts) -> Result<bool>;
	async fn task_rows(&mut self, threads: &[Uuid], cursor: u64) -> Result<Vec<Task>>;
	async fn event_rows(
		&mut self,
		threads: &[Uuid],
		include_marketplace: bool,
		cursor: i64,
		page_size: usize,
	) -> Result<Vec<Event>>;
	async fn message_rows(&mut self, thread: Uuid, offset: u64) -> Result<Vec<Message>>;
	async fn thread_row(&mut self, id: Uuid) -> Result<Thread>;
	async fn thread_tasks(&mut self, id: Uuid) -> Result<Vec<Task>>;

	/// Retain the independent durable read-membership commit before external disclosure.
	async fn track(&mut self, snapshot: &ThreadSnapshot) -> Result<()>;
}
