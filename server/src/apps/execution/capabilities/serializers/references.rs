use serde::{Deserialize, Serialize};
// Serializable references contracts.
use crate::apps::execution::capabilities::services::core::contracts::*;

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Upload {
	pub idempotency_key: Uuid,
	pub name: String,
	pub media_type: String,
	pub size: u64,
	pub digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
	pub offset: u64,
	/// Base64 bytes: exactly 4 MiB per sequential chunk, except the final chunk.
	pub data: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct Reference {
	pub reference_id: Uuid,
	pub revision: i64,
	pub state: String,
	pub name: String,
	pub media_type: String,
	pub size: u64,
	pub digest: String,
	pub uploaded_bytes: u64,
	pub original: Option<FileEntry>,
	pub extraction: Option<FileEntry>,
	pub extraction_state: Option<String>,
}

use uuid::Uuid;

pub use aidash_domain::capabilities::ReferenceAttachment as Attachment;
