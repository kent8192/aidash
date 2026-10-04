use serde::{Deserialize, Serialize};
// Serializable cleanup contracts.
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
	Keep,
	Recoverable,
	Irreversible,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Cleanup {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	pub choice: Choice,
	pub confirmation_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Restore {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	pub snapshot_id: Uuid,
	pub thread_id: Uuid,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct RestoreNewThread {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	pub snapshot_id: Uuid,
	pub content: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ManagedArea {
	pub area_id: Uuid,
	pub workspace_id: Uuid,
	pub thread_id: Uuid,
	pub agent_id: String,
	pub owner: String,
	pub state: String,
	pub revision: i64,
	pub generation: i64,
	pub files: usize,
	pub bytes: u64,
	pub snapshot_id: Option<Uuid>,
	pub recovery_expires_at: Option<DateTime<Utc>>,
	pub cleanup_operation_id: Option<Uuid>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ManagementPage {
	pub items: Vec<ManagedArea>,
	pub next_cursor: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct CleanupResult {
	pub operation_id: Uuid,
	pub area_id: Uuid,
	pub state: String,
	pub revision: i64,
	pub recovery_expires_at: Option<DateTime<Utc>>,
}

use uuid::Uuid;
