//! Immutable local and remote file-share identities.
use serde::{Deserialize, Serialize};
// Serializable sharing contracts.

pub use crate::media::Selection;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipient {
	pub node_id: String,
	pub agent_id: String,
	pub agent_version: String,
	pub thread_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Share {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	pub files: Vec<Selection>,
	pub recipient: Recipient,
}

use uuid::Uuid;
