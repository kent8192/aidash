use serde::{Deserialize, Serialize};
// Serializable python contracts.

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Python {
	pub idempotency_key: Uuid,
	pub code: String,
	/// The working area revision: take it from the previous result's `revision`; a new area starts at 1.
	pub expected_revision: i64,
	pub expected_session_id: Option<Uuid>,
	/// Seconds the cell may run once it starts. It is then stopped, the session resets, and its changes under /work are saved.
	pub timeout_seconds: Option<u64>,
	/// How long the Run waits for completion; zero returns immediately.
	pub wait_seconds: Option<u64>,
}

use uuid::Uuid;
