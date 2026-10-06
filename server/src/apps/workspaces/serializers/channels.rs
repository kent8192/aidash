use serde::Deserialize;
// Serializable contracts for collaboration.

use reinhardt::Validate;
use schemars::JsonSchema;

#[derive(Debug, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelMessageInput {
	#[validate(length(max = 64000))]
	pub content: String,
	pub thread_id: Option<Uuid>,
	pub idempotency_key: Uuid,
	#[serde(default)]
	pub attachment_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelThreadInput {
	pub root_message_id: Uuid,
}

#[derive(Debug, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelHistoryQuery {
	pub thread_id: Option<Uuid>,
	pub before: Option<Uuid>,
	#[serde(default = "default_limit")]
	#[validate(range(min = 1, max = 100))]
	pub limit: u16,
}

#[derive(Debug, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelAttachmentUploadQuery {
	#[validate(length(min = 1, max = 255))]
	pub filename: String,
	#[validate(length(min = 1, max = 128))]
	pub media_type: String,
	pub idempotency_key: Uuid,
}

pub(crate) fn default_limit() -> u16 {
	40
}

use uuid::Uuid;

pub use aidash_domain::workspaces::channels::{
	ChannelAttachment, ChannelMessage, ChannelMessagePage, ChannelThread,
};
