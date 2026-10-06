use serde::{Deserialize, Serialize};
// Serializable python contracts.

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Python {
	pub idempotency_key: Uuid,
	pub code: String,
	pub expected_revision: i64,
	pub expected_session_id: Option<Uuid>,
	pub timeout_seconds: Option<u64>,
}

use uuid::Uuid;
