//! Input records replayed into the next inference context.
use serde::Deserialize;
use uuid::Uuid;

/// Reserved executor key prefix for results sent by a Subnode member.
pub const SUBNODE_RESULT_PREFIX: &str = "subnode-result:";

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct RunInput {
	pub seq: i64,
	pub sender: String,
	pub content: String,
	pub idempotency_key: String,
	pub message_id: Option<Uuid>,
	pub reference_only: bool,
	/// A Subnode result is data, never a person's instruction.
	#[serde(default)]
	pub is_data: bool,
}

pub mod remote;
