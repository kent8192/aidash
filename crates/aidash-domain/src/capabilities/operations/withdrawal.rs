//! Withdrawal narrows accepted authority without treating a dispatched writer as safe.
use serde_json::{Value, json};
use uuid::Uuid;
/// Why an operation's authority ended, recorded in its result. A refused request
/// keeps answering that its authority was revoked, not that it expired.
pub const AUTHORITY_WITHDRAWN: &str = "AUTHORITY_WITHDRAWN";
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
/// What a cancel proved about an operation's writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriterStop {
	Unconfirmed,
	Confirmed,
	/// It stopped before it started, so the command changed nothing.
	NeverRan,
}
impl WriterStop {
	/// Only a settled outcome can say that nothing ran: an `uncertain` one lost
	/// track of its command, and `absent` says nothing about it at all.
	pub fn from_reply(reply: &Value) -> Self {
		if reply["termination_confirmed"] != true {
			Self::Unconfirmed
		} else if reply["executed"] == false
			&& matches!(
				reply["status"].as_str(),
				Some("completed" | "cancelled" | "failed")
			) {
			Self::NeverRan
		} else {
			Self::Confirmed
		}
	}
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
	/// A withdrawal has already recorded that this operation's authority ended.
	/// The writer's stop may still be unconfirmed.
	pub fn marked_withdrawn(&self) -> bool {
		self.result["error"] == AUTHORITY_WITHDRAWN
	}
	pub fn undelivered_refusal(&self) -> Option<Value> {
		if !self.active() {
			return None;
		}
		super::undelivered_refusal(&self.result, &self.digest, self.runner_instance.as_deref())
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
			area_state: (stopped
				&& self.state == "uncertain"
				&& self.area_state == "uncertain"
				&& self.area_epoch == self.epoch
				&& self.area_generation == self.generation)
				.then_some("active"),
		}
	}
	pub fn change(&self, stop: WriterStop) -> Change {
		let stopped = stop != WriterStop::Unconfirmed;
		let never_dispatched = self.never_dispatched();
		let never_ran = never_dispatched || stop == WriterStop::NeverRan;
		// A confirmed stop reopens the last saved Area even if the output was lost.
		let fenced_area = self.area_state == "uncertain";
		let mut result = self.result.clone();
		result["termination_confirmed"] = json!(stopped);
		result["effects_may_have_occurred"] = json!(!never_ran);
		result["error"] = json!(AUTHORITY_WITHDRAWN);
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
				&& (self.area_state == "running" || fenced_area && stopped))
				.then_some(if never_ran {
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
