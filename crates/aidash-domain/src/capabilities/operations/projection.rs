//! Operation output projection borrows its durable input and result without native row types.
use super::MountedFile as FileEntry;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
pub struct OperationView<'a> {
	pub id: Uuid,
	pub area_id: Uuid,
	pub kind: &'a str,
	pub state: &'a str,
	pub generation: i64,
	pub revision: i64,
	pub epoch: i64,
	pub policy_revision: i64,
	pub input: &'a serde_json::Value,
	pub result: &'a serde_json::Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationResult {
	pub operation_id: Uuid,
	pub kind: String,
	pub status: String,
	pub area_id: Uuid,
	pub generation: i64,
	pub revision: i64,
	pub epoch: i64,
	pub policy_revision: i64,
	pub termination_confirmed: bool,
	pub writer_frozen: bool,
	pub session_id: Option<Uuid>,
	pub displays: Vec<FileEntry>,
	pub exit_code: Option<i64>,
	pub output: String,
	pub next_offset: Option<usize>,
	pub truncated: bool,
	pub effects_may_have_occurred: bool,
	pub error: Option<crate::capabilities::errors::CapabilityError>,
}
