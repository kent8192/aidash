//! Durable channel conversations; message creation does not authorize execution.
pub(crate) mod access;
pub mod api;
mod attachments;
mod history;
pub(crate) mod threads;

use crate::domain::Message;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct ChannelThread {
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub root_message_id: Uuid,
	pub created_by: String,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChannelMessage {
	pub message: Message,
	pub thread_id: Option<Uuid>,
	pub is_thread_root: bool,
	pub attachments: Vec<ChannelAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChannelAttachment {
	pub id: Uuid,
	pub filename: String,
	pub media_type: String,
	pub size_bytes: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ChannelMessagePage {
	/// Each page is chronological; next_before retrieves an older page.
	pub messages: Vec<ChannelMessage>,
	pub next_before: Option<Uuid>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelMessageInput {
	pub content: String,
	pub thread_id: Option<Uuid>,
	pub idempotency_key: Uuid,
	#[serde(default)]
	pub attachment_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelThreadInput {
	pub root_message_id: Uuid,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct ChannelHistoryQuery {
	pub thread_id: Option<Uuid>,
	pub before: Option<Uuid>,
	#[serde(default = "default_limit")]
	pub limit: u16,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct ChannelAttachmentUploadQuery {
	pub filename: String,
	pub media_type: String,
	pub idempotency_key: Uuid,
}

fn default_limit() -> u16 {
	40
}
