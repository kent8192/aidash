//! Immutable mounted inputs and operation admission constraints are independent of storage.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileScope {
	Working,
	References,
	Received,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountedFile {
	pub file_id: Uuid,
	pub path: String,
	pub digest: String,
	pub size: u64,
	pub media_type: String,
	pub scope: FileScope,
	pub provenance: Value,
}
pub const DEFAULT_BACKGROUND_MAXIMUM_SECONDS: u64 = 14_400;
pub const BACKGROUND_MAXIMUM_SECONDS: u64 = 86_400;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellRequest {
	pub idempotency_key: Uuid,
	pub command: String,
	pub timeout_seconds: Option<u64>,
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub background: bool,
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub notify_on_exit: bool,
	pub expected_revision: i64,
}
#[derive(Clone, Copy)]
pub struct AdmissionLimits {
	pub admission: bool,
	pub working_bytes: u64,
	pub command_bytes: usize,
	pub operation_seconds: u64,
	pub maximum_seconds: u64,
}
#[derive(Clone)]
pub struct AreaSnapshot {
	pub id: Uuid,
	pub thread_id: Uuid,
	pub state: String,
	pub epoch: i64,
	pub revision: i64,
}
impl ShellRequest {
	pub fn seconds(&self, limits: AdmissionLimits) -> Result<u64> {
		let seconds = self.timeout_seconds.unwrap_or(limits.operation_seconds);
		if self.command.is_empty()
			|| self.command.len() > limits.command_bytes
			|| seconds == 0
			|| seconds > limits.maximum_seconds
		{
			return Err(Error::Invalid("INVALID_SHELL_LIMIT".into()));
		}
		Ok(seconds)
	}
	pub fn digest(&self, kind: &str, run: Uuid, extra: &Value) -> String {
		crate::registry::rules::digest(&json!([kind, run, self, extra]))
	}
	pub fn key(&self, run: Uuid) -> String {
		format!("core:{run}:{}", self.idempotency_key)
	}
}
pub fn has_input_path_collision(files: &[MountedFile]) -> bool {
	let mut seen = std::collections::BTreeSet::new();
	for file in files {
		let scope = match file.scope {
			FileScope::Working => 0,
			FileScope::References => 1,
			FileScope::Received => 2,
		};
		if !seen.insert((scope, file.path.as_str())) {
			return true;
		}
	}
	false
}
pub fn request_validation_failure(
	input_budget: u64,
	files: &[MountedFile],
) -> Option<(&'static str, &'static str)> {
	if has_input_path_collision(files) {
		return Some((
			"INPUT_PATH_COLLISION",
			"Mounted input files contain a duplicate path in the same scope.",
		));
	}
	let exceeds_quota = files
		.iter()
		.try_fold(0_u64, |total, file| total.checked_add(file.size))
		.is_none_or(|size| size >= input_budget);
	if exceeds_quota {
		Some((
			"WORKING_QUOTA_EXCEEDED",
			"Mounted files exhaust the configured input budget.",
		))
	} else {
		None
	}
}
impl MountedFile {
	/// A Reference file is read through the file tools.
	pub fn is_reference_file(&self) -> bool {
		matches!(self.scope, FileScope::References) && self.provenance["kind"] == "reference"
	}
}
/// A lost outcome leaves the last saved Area intact. It remains fenced while
/// the original writer may still run, and reopens once its stop is proven.
pub fn lost_area_state(_kind: &str, writer_held: bool) -> &'static str {
	if !writer_held { "active" } else { "uncertain" }
}
pub fn available(state: &str) -> Result<()> {
	if state != "active" {
		return Err(Error::Conflict(
			if state == "running" {
				"AREA_BUSY"
			} else {
				"AREA_UNAVAILABLE"
			}
			.into(),
		));
	}
	Ok(())
}
#[cfg(test)]
mod tests;

/// The polling boundary discloses only operations belonging to the same active scope.
#[derive(Clone)]
pub struct OperationState {
	pub area_id: Uuid,
	pub run_id: Uuid,
	pub principal: String,
	pub kind: String,
	pub state: String,
	/// Runs in the Execution Pod of the Run's Scratch Space and holds no
	/// Working Area writer, so its cancellation leaves the Area alone (#745).
	pub pod: bool,
}
/// `input.runtime` of an operation that runs in an Execution Pod (#745).
pub const POD_RUNTIME: &str = "pod";
/// Whether an operation's recorded input says it runs in an Execution Pod.
pub fn runs_in_pod(input: &Value) -> bool {
	input["runtime"] == POD_RUNTIME
}
/// The epoch of a Pod operation. It holds no Working Area writer, so no Area
/// epoch ever equals it and withdrawal or cancellation never reopens the Area.
pub const POD_EPOCH: i64 = -1;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cancellation {
	pub state: &'static str,
	pub result: Option<Value>,
	pub never_dispatched: bool,
}
impl OperationState {
	pub fn visible_to(&self, area: Uuid, run: Uuid, principal: &str, kind: &str) -> bool {
		self.area_id == area
			&& self.run_id == run
			&& self.principal == principal
			&& self.kind == kind
	}
	pub fn cancellation(&self) -> Option<Cancellation> {
		if matches!(
			self.state.as_str(),
			"completed" | "cancelled" | "failed" | "withdrawn"
		) {
			return None;
		}
		let never_dispatched = self.state == "prepared";
		Some(Cancellation {
			state: if never_dispatched {
				"cancelled"
			} else {
				"cancelling"
			},
			result: never_dispatched
				.then(|| json!({"termination_confirmed":true,"effects_may_have_occurred":false})),
			never_dispatched,
		})
	}
}

/// What a cancel asks the Runner to refuse, if the operation may not have
/// reached it. `dispatch_pending` records the intent to send, not whether the
/// send arrived: the Runner may hold the operation already, or receive it later
/// from a send its caller gave up on. Only the Runner can settle that, and only
/// the one this intent was recorded against, so it compares itself to `instance`
/// and refuses any later send under the lock that accepts one.
pub fn undelivered_refusal(result: &Value, digest: &str, instance: Option<&str>) -> Option<Value> {
	if result["dispatch_pending"] != true {
		return None;
	}
	instance.map(|instance| json!({"digest":digest,"instance":instance}))
}

pub mod withdrawal;

pub mod runner;

pub mod reconciliation;

pub mod processing;

pub mod projection;
