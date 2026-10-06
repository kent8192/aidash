//! Every operation borrows the same current authorization and write transaction.
use crate::{Result, workspaces::channels::AttachmentUpload};
use aidash_domain::{
	Message,
	workspaces::channels::{AttachmentState, ChannelAttachment, ChannelThread},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct MessageContext {
	pub thread_id: Option<Uuid>,
	pub attachment_digest: String,
}
#[derive(Debug, Clone)]
pub struct AttachmentLink {
	pub attachment: ChannelAttachment,
	pub message_id: Option<Uuid>,
}
pub struct HistoryRow {
	pub message: Message,
	pub reply_thread_id: Option<Uuid>,
	pub root_thread_id: Option<Uuid>,
}

/// The outer owner admits the actor, retains policy/credential locks, and finishes
/// exactly once. Failures roll back writes while retaining the existing denial audit.
#[async_trait]
pub trait ChannelScope: Send {
	fn sender(&self) -> String;
	fn principal(&self) -> String;
	async fn message(&mut self, workspace: Uuid, id: Uuid) -> Result<Message>;
	async fn visible(&mut self, message: &Message) -> Result<bool>;
	async fn thread_visible(&mut self, id: Uuid) -> Result<()>;
	async fn thread(&mut self, workspace: Uuid, id: Uuid) -> Result<Option<ChannelThread>>;
	async fn reply_thread(&mut self, message: Uuid) -> Result<Option<Uuid>>;
	async fn insert_thread(
		&mut self,
		workspace: Uuid,
		root: Uuid,
		sender: &str,
	) -> Result<Option<ChannelThread>>;
	async fn existing_thread(&mut self, workspace: Uuid, root: Uuid) -> Result<ChannelThread>;
	async fn thread_event(&mut self, workspace: Uuid, root: Uuid, thread: Uuid) -> Result<()>;
	async fn append_message(
		&mut self,
		workspace: Uuid,
		sender: &str,
		content: &str,
		key: &str,
	) -> Result<Message>;
	async fn record_context(
		&mut self,
		workspace: Uuid,
		message: Uuid,
		thread: Option<Uuid>,
		digest: &str,
	) -> Result<MessageContext>;
	async fn root_thread(&mut self, message: Uuid) -> Result<Option<Uuid>>;
	async fn legacy_attachment_positions(
		&mut self,
		workspace: Uuid,
		message: Uuid,
	) -> Result<Vec<(Uuid, i32)>>;
	/// Lock attachment records in UUID order, constrained by workspace and principal.
	async fn attachment_links(
		&mut self,
		workspace: Uuid,
		ids: &[Uuid],
		principal: &str,
	) -> Result<Vec<AttachmentLink>>;
	async fn bind_attachment(
		&mut self,
		workspace: Uuid,
		message: Uuid,
		id: Uuid,
		principal: &str,
		position: i32,
	) -> Result<()>;
	async fn insert_attachment(
		&mut self,
		workspace: Uuid,
		principal: &str,
		input: &AttachmentUpload<'_>,
		digest: &str,
		content: &[u8],
	) -> Result<Option<AttachmentState>>;
	/// Replay lookup must hold a row update lock until the scope finishes.
	async fn existing_attachment(
		&mut self,
		workspace: Uuid,
		principal: &str,
		key: Uuid,
	) -> Result<AttachmentState>;
	async fn attachment(&mut self, workspace: Uuid, id: Uuid) -> Result<Option<AttachmentState>>;
	/// Select at most 100 newest rows below the cursor, excluding tombstoned roots.
	async fn history_rows(
		&mut self,
		workspace: Uuid,
		thread: Option<&ChannelThread>,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<HistoryRow>>;
	async fn message_attachments(
		&mut self,
		workspace: Uuid,
		ids: &[Uuid],
	) -> Result<HashMap<Uuid, Vec<ChannelAttachment>>>;
}
