//! Withdrawal narrows accepted authority without treating a dispatched writer as safe.
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Clone)]
pub struct Snapshot {
	pub operation_id: Uuid,
	pub state: String,
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
		Change {
			state: if stopped { "withdrawn" } else { "cancelling" },
			result: json!({"termination_confirmed":stopped,"effects_may_have_occurred":!never_dispatched,"error":"AUTHORITY_WITHDRAWN"}),
			area_state: (self.area_epoch == self.epoch
				&& self.area_generation == self.generation
				&& self.area_state == "running")
				.then_some(if never_dispatched {
					"active"
				} else {
					"uncertain"
				}),
		}
	}
}
#[cfg(test)]
mod tests;
