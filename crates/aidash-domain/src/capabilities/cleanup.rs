//! Explicit file retention and recovery preserve the authorized generation fence.
use super::records::Record;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
	Keep,
	Recoverable,
	Irreversible,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cleanup {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	pub choice: Choice,
	pub confirmation_id: Option<Uuid>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Restore {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	pub snapshot_id: Uuid,
	pub thread_id: Uuid,
}
#[derive(Debug, Serialize)]
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
#[derive(Debug, Serialize)]
pub struct ManagementPage {
	pub items: Vec<ManagedArea>,
	pub next_cursor: Option<Uuid>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct CleanupResult {
	pub operation_id: Uuid,
	pub area_id: Uuid,
	pub state: String,
	pub revision: i64,
	pub recovery_expires_at: Option<DateTime<Utc>>,
}
pub fn result(record: &Record) -> Option<CleanupResult> {
	Some(CleanupResult {
		operation_id: record.id,
		area_id: record.area_id?,
		state: record.state.clone(),
		revision: record.data["area_revision"].as_i64()?,
		recovery_expires_at: record.expires_at,
	})
}
