use serde::{Deserialize, Serialize};
// Serializable packages contracts.

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Wheel {
	pub outbound_operation_id: Uuid,
	pub filename: String,
	pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Install {
	pub idempotency_key: Uuid,
	/// The working area revision: take it from the previous result's `revision`; a new area starts at 1.
	pub expected_revision: i64,
	pub wheels: Vec<Wheel>,
	/// Seconds the install may run once it starts. It is then stopped, and its changes under /work are saved. Raise it if an install ran out of time.
	pub timeout_seconds: Option<u64>,
}

use uuid::Uuid;
