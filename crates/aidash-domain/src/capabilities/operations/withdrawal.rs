//! Withdrawal narrows accepted authority without treating a dispatched writer as safe.
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Clone)]
pub struct Snapshot {
	pub operation_id: Uuid,
	/// Operation kind, such as `shell` or a Completion Gate `verification`.
	pub kind: String,
	pub state: String,
	/// Preserve saved command output while narrowing authority.
	pub result: Value,
	pub epoch: i64,
	pub generation: i64,
	pub area_epoch: i64,
	pub area_generation: i64,
	pub area_state: String,
}
pub struct Change {
	pub state: &'static str,
	pub result: Value,
	pub area_state: Option<&'static str>,
}
impl Snapshot {
	pub fn active(&self) -> bool {
		matches!(
			self.state.as_str(),
			"prepared" | "submitted" | "running" | "cancelling"
		)
	}
	pub fn never_dispatched(&self) -> bool {
		self.state == "prepared"
	}
	pub fn change(&self, stopped: bool) -> Change {
		let never_dispatched = self.never_dispatched();
		// A Check never exports `/work`, so once its writer is confirmed
		// stopped the Area it fenced on an earlier pass is usable again.
		let fenced_check = self.kind == super::VERIFICATION && self.area_state == "uncertain";
		let mut result = self.result.clone();
		result["termination_confirmed"] = json!(stopped);
		result["effects_may_have_occurred"] = json!(!never_dispatched);
		result["error"] = json!("AUTHORITY_WITHDRAWN");
		if !never_dispatched {
			result
				.as_object_mut()
				.expect("operation result object")
				.entry("runner_acknowledged")
				.or_insert(json!(false));
		}
		Change {
			state: if stopped { "withdrawn" } else { "cancelling" },
			result,
			area_state: (self.area_epoch == self.epoch
				&& self.area_generation == self.generation
				&& (self.area_state == "running" || fenced_check && stopped))
				.then_some(if never_dispatched {
					"active"
				} else {
					super::lost_area_state(&self.kind, !stopped)
				}),
		}
	}
}
#[cfg(test)]
mod tests;
