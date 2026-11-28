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

/// Open a delivery from a Run executing for another Home (ADR 0139, decision 20). It names the
/// grant, the admission and the Run's call key, and what is sent; where it lands is the Home's.
#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeliveryOpen {
	pub(crate) grant_id: Uuid,
	pub(crate) admission_id: Uuid,
	pub(crate) key: String,
	pub(crate) purpose: aidash_domain::federation::delivery::Purpose,
	pub(crate) files: Vec<aidash_domain::federation::delivery::File>,
}

/// One chunk of one file of an open delivery. Chunks are at most 4 MiB, a multiple of 4 MiB
/// apart, base64 encoded.
#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeliveryChunk {
	pub(crate) grant_id: Uuid,
	pub(crate) admission_id: Uuid,
	pub(crate) key: String,
	/// The `thread_message` tool the Run called.
	pub(crate) tool: String,
	pub(crate) file: usize,
	pub(crate) offset: u64,
	pub(crate) data: String,
}

use uuid::Uuid;

use serde_json::Value;
