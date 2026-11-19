//! Working session identity is independent of its row codec and HTTP projection.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Area {
	pub id: Uuid,
	pub tenant: String,
	pub home_node: String,
	pub thread_id: Uuid,
	pub agent_id: String,
	pub owner: String,
	pub generation: i64,
	pub revision: i64,
	pub epoch: i64,
	pub state: String,
	pub manifest: Value,
	pub constraints: Value,
	pub next_sequence: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRun {
	pub run_id: Uuid,
	pub sequence: i64,
	pub phase: String,
	pub control: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatus {
	pub area_id: Uuid,
	pub last_run_id: Option<Uuid>,
	pub last_agent_version: Option<String>,
	pub active_run_id: Option<Uuid>,
	pub queue: Vec<SessionRun>,
}
/// The constraint kind of a Working Area a receiving Node keeps for Runs executing for another
/// Home (ADR 0139). It records the Home, whose Thread this Node does not hold, so reads of
/// the Area are authorized by the Run's admission and the Area's owner instead.
pub const REMOTE_HOME: &str = "remote_home";
pub fn remote_home_constraint(home_node: &str) -> Value {
	serde_json::json!({"kind": REMOTE_HOME, "home_node": home_node})
}
impl Area {
	/// Whether this Area belongs to Runs executing here for another Home.
	pub fn is_remote_home(&self) -> bool {
		self.constraints
			.as_array()
			.is_some_and(|sources| sources.iter().any(|source| source["kind"] == REMOTE_HOME))
	}
	/// Whether this Area's files may leave for `recipient_node`, a Node address. An Area that
	/// belongs to Runs executing here for another Home, with or without a Reference set, sends
	/// files to that Home only (ADR 0114 amendment, ADR 0139): the receiving Tenant consented to
	/// that by listing the tool, and to no other destination. Any other Area is not decided
	/// here; the caller's own checks apply, which refuse a Reference set.
	pub fn may_send_to(&self, recipient_node: &str) -> bool {
		!self.is_remote_home() || recipient_node == self.home_node
	}
	/// Refuse an Area of the other kind than the Run needs. A receiving Area and a local one
	/// share their key (Tenant, Home Node, Thread, Agent, owner) when the Home is a sibling Node
	/// of this Deployment (#1008), so the row found for a key may be the other kind's; a Run
	/// never works in it, and neither kind reaches the other's files.
	pub fn require_kind(&self, receiving: bool) -> crate::Result<()> {
		if self.is_remote_home() == receiving {
			Ok(())
		} else {
			Err(crate::Error::Conflict(
				"AREA_UNAVAILABLE: this Thread's Working Area for the Agent and person serves Runs of another Node"
					.into(),
			))
		}
	}
}
pub fn received_scope_matches(subjects: &[String], owner: &str, agent: &str) -> bool {
	subjects.first().map(String::as_str) == Some(owner)
		&& subjects.last().map(String::as_str) == Some(agent)
}
pub fn reusable_context(state: &str) -> bool {
	!matches!(
		state,
		"deleted" | "recoverable" | "uncertain" | "cleaning" | "cleanup_failed" | "retained"
	)
}
#[cfg(test)]
mod tests;
