//! Working session identity is independent of its row codec and HTTP projection.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Area {
	pub id: Uuid,
	pub tenant: String,
	pub home_node: String,
	pub workspace_id: Uuid,
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
