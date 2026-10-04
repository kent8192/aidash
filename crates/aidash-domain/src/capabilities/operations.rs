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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellRequest {
	pub idempotency_key: Uuid,
	pub command: String,
	pub timeout_seconds: Option<u64>,
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
	pub workspace_id: Uuid,
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
pub fn package_files(extra: &Value) -> std::result::Result<Vec<MountedFile>, serde_json::Error> {
	serde_json::from_value(extra.get("package_files").cloned().unwrap_or(json!([])))
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
pub fn request_size(files: &[MountedFile]) -> Result<u64> {
	files.iter().try_fold(0_u64, |total, file| {
		total
			.checked_add(file.size)
			.ok_or_else(|| Error::Conflict("WORKING_QUOTA_EXCEEDED".into()))
	})
}
pub fn request_validation_failure(
	working_bytes: u64,
	files: &[MountedFile],
) -> Option<(&'static str, &'static str)> {
	if has_input_path_collision(files) {
		return Some((
			"INPUT_PATH_COLLISION",
			"Mounted input files contain a duplicate path in the same scope.",
		));
	}
	let exceeds_quota = match request_size(files) {
		Ok(size) => {
			size > working_bytes
				|| files
					.iter()
					.filter(|file| !matches!(file.scope, FileScope::Working))
					.map(|file| file.size)
					.sum::<u64>() >= working_bytes
		}
		Err(_) => true,
	};
	if exceeds_quota {
		Some((
			"WORKING_QUOTA_EXCEEDED",
			"Mounted files exceed the configured quota or leave no writable capacity.",
		))
	} else {
		None
	}
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
}
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
			&& (self.kind == kind || (kind == "code_interpreter" && self.kind == "python_install"))
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

pub mod withdrawal;

pub mod runner;

pub mod reconciliation;

pub mod processing;
