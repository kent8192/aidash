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
impl OperationView<'_> {
	/// When a running command's Time Limit ends: the start the runner reported
	/// plus the limit. An operation that has not started or has ended has none.
	pub fn command_deadline(&self) -> Option<chrono::DateTime<chrono::Utc>> {
		let started = self.result["started_at"].as_f64()?;
		let seconds = self.input["seconds"].as_f64()?;
		chrono::DateTime::from_timestamp_micros(((started + seconds) * 1_000_000.0) as i64)
	}
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
	#[serde(default, skip_serializing_if = "String::is_empty")]
	pub output: String,
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub output_unchanged: bool,
	pub next_offset: Option<usize>,
	/// When a running command will be stopped. Absent before it starts and after it ends.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub operation_deadline: Option<chrono::DateTime<chrono::Utc>>,
	pub truncated: bool,
	pub effects_may_have_occurred: bool,
	pub error: Option<crate::capabilities::errors::CapabilityError>,
	/// Seconds since the Runner last obtained the output preview.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub output_stale_seconds: Option<u64>,
	/// How long a Completion Gate Check's command ran, in milliseconds, as the
	/// runner measured it. Absent for every other operation.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub runtime_millis: Option<u64>,
	/// Whether the operation's deadline, rather than its command, ended it, as
	/// the runner observed. A Completion Gate Check says so either way, and is
	/// silent when the runner could not tell; any other operation carries it
	/// only when true.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub deadline_expired: Option<bool>,
}
