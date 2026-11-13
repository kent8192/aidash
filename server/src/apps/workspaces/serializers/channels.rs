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
	/// Exact File versions this message cites, in composer order.
	#[serde(default)]
	pub file_versions: Vec<CitedFileVersion>,
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
	/// An Agent's Image Description of an image it posts (#622). Never read
	/// from the query string: people describe an upload through the JSON
	/// description endpoint, so the words stay out of URLs and access logs.
	#[serde(skip)]
	#[schemars(skip)]
	pub description: Option<String>,
}

/// An Image Description, written by the person who uploaded the image, before
/// or after posting it. Its limit of 2,000 characters is checked with the
/// attachment's media type.
#[derive(Debug, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelAttachmentDescription {
	pub description: String,
}

pub(crate) fn default_limit() -> u16 {
	40
}

use uuid::Uuid;

pub use aidash_domain::workspaces::channels::{
	ChannelAttachment, ChannelCitation, ChannelMessage, ChannelMessagePage, ChannelThread,
	CitedFileVersion,
};
