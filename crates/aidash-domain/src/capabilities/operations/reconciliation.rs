//! Reconciliation retains a complete durable operation snapshot and its writer fence.
use crate::{RunControl, RunMetadata};
use serde_json::Value;
use uuid::Uuid;
#[derive(Clone)]
pub struct Snapshot {
	pub id: Uuid,
	pub area_id: Uuid,
	pub run_id: Uuid,
	pub tenant: String,
	pub principal: String,
	pub credential_id: Uuid,
	pub subjects: Value,
	pub digest: String,
	pub kind: String,
	pub state: String,
	pub epoch: i64,
	pub generation: i64,
	pub revision: i64,
	pub policy_revision: i64,
	pub input: Value,
	pub result: Value,
	pub runner_instance: Option<String>,
}
#[derive(Clone)]
pub struct Area {
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub generation: i64,
	pub revision: i64,
	pub epoch: i64,
	pub manifest: Value,
}
#[derive(Clone, Copy)]
pub struct Limits {
	pub admission: bool,
	pub working_bytes: u64,
	pub output_bytes: u64,
	pub read_bytes: usize,
}
impl Snapshot {
	pub fn active(&self) -> bool {
		matches!(
			self.state.as_str(),
			"prepared" | "submitted" | "running" | "cancelling"
		)
	}
	pub fn writer_matches(&self, area: &Area) -> bool {
		area.epoch == self.epoch
			&& area.generation == self.generation
			&& area.revision == self.revision
	}
	pub fn cancelling(&self, limits: Limits, run: &RunMetadata) -> bool {
		!limits.admission
			|| self.state == "cancelling"
			|| run.control == RunControl::Cancelled
			|| run.phase.is_terminal()
	}
	pub fn runner_changed(&self, instance: &str, cancelling: bool) -> bool {
		if cancelling {
			self.runner_instance.as_deref() != Some(instance)
		} else {
			self.runner_instance
				.as_deref()
				.is_some_and(|old| old != instance)
		}
	}
}
#[cfg(test)]
mod tests;
