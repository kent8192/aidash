//! Withdrawal narrows accepted authority without treating a dispatched writer as safe.
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Clone)]
pub struct Snapshot {
	pub operation_id: Uuid,
	pub digest: String,
	pub runner_instance: Option<String>,
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
	pub state: String,
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
	/// Node cleanup preserves the semantic outcome of an already terminal command.
	pub fn confirm_terminal_stop(&self, stopped: bool) -> Change {
		let mut result = self.result.clone();
		result["termination_confirmed"] = json!(stopped);
		result
			.as_object_mut()
			.expect("operation result object")
			.entry("runner_acknowledged")
			.or_insert(json!(false));
		Change {
			state: self.state.clone(),
			result,
			area_state: None,
		}
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
			state: if stopped { "withdrawn" } else { "cancelling" }.into(),
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
/// Physical deletion requires both the original writer stop and saved-output ACK.
/// Uncertain outcomes never qualify through the never-dispatched shortcut.
pub fn safe_to_erase(state: &str, result: &Value, instance: Option<&str>) -> bool {
	let confirmed =
		result["termination_confirmed"] == true && result["runner_acknowledged"] == true;
	match state {
		"completed" | "cancelled" | "failed" | "withdrawn" => {
			confirmed || (instance.is_none() && result["effects_may_have_occurred"] == false)
		}
		"uncertain" => confirmed,
		_ => false,
	}
}
#[cfg(test)]
mod tests;
