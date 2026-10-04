use serde::{Deserialize, Serialize};
// Serializable durable contracts.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
	pub version: u32,
	pub node_id: String,
	pub run_id: Uuid,
	pub activation_id: Uuid,
	pub generation: i64,
}

use uuid::Uuid;
