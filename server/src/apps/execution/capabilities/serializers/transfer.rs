use serde::{Deserialize, Serialize};
// Serializable transfer contracts.
use crate::apps::execution::capabilities::services::core::{contracts::*, sharing::Recipient};
use crate::registry::EntityRef;
use chrono::{DateTime, Utc};

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Description {
	pub protocol: String,
	pub transfer_id: Uuid,
	pub source_node: String,
	pub target: Recipient,
	pub source_tenant: String,
	pub source_subject: String,
	pub source_agent: EntityRef,
	pub input_digest: String,
	pub manifest_digest: String,
	pub files: Vec<FileEntry>,
	pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Identity {
	pub transfer_id: Uuid,
	pub input_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Requester {
	pub tenant: String,
	pub subject: String,
	#[serde(default)]
	pub cursor: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
	pub transfer_id: Uuid,
	pub input_digest: String,
	pub file: usize,
	pub offset: u64,
	pub data: String,
}

use uuid::Uuid;
