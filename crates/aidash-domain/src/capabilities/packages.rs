//! Offline package identities refer to explicitly approved immutable broker artifacts.
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wheel {
	pub outbound_operation_id: Uuid,
	pub filename: String,
	pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Install {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	pub wheels: Vec<Wheel>,
	pub timeout_seconds: Option<u64>,
}
