//! Collaboration contracts and target invariants independent of persistence.
use crate::{
	Conversation, Error, Result, Task, Workspace,
	federation::Delegation,
	registry::{EntityRef, Entry},
};
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Serialize, JsonSchema)]
pub struct ConversationResponse {
	pub conversation: Conversation,
	pub workspace: Workspace,
	pub task: Task,
	pub delegation: Delegation,
}

pub fn validate_conversation_target(entry: &Entry, expected_kind: &str) -> Result<()> {
	if entry.kind != expected_kind || !matches!(expected_kind, "agent" | "cluster") {
		return Err(Error::Invalid(
			"conversation target must be an agent or cluster".into(),
		));
	}
	Ok(())
}
pub fn conversation_agent(entry: &Entry, target: &EntityRef) -> Result<EntityRef> {
	if entry.kind == "cluster" {
		serde_json::from_value(entry.config["coordinator"].clone()).map_err(|_| {
			Error::Invalid("cluster requires an explicit coordinator agent reference".into())
		})
	} else {
		Ok(target.clone())
	}
}

pub mod channels;

#[derive(Serialize, JsonSchema)]
pub struct TaskPage {
	pub tasks: Vec<Task>,
	pub next_offset: Option<u64>,
}
