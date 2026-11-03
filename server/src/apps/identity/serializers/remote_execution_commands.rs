use serde::Deserialize;
// Serializable remote execution commands contracts.

#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
	pub(crate) grant_id: Uuid,
	pub(crate) admission_id: Uuid,
	pub(crate) operation: String,
	pub(crate) data: Value,
}

#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct AttachmentInput {
	pub(crate) grant_id: Uuid,
	pub(crate) admission_id: Uuid,
	pub(crate) attachment_id: Uuid,
}

#[derive(serde::Serialize, schemars::JsonSchema)]
pub(crate) struct AttachmentResponse {
	pub(crate) filename: String,
	pub(crate) media_type: String,
	pub(crate) size_bytes: i64,
	pub(crate) message_id: Uuid,
	pub(crate) sha256: String,
	pub(crate) content_base64: String,
}

use uuid::Uuid;

use serde_json::Value;
