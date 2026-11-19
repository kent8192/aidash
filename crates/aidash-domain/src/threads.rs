//! Collaboration contracts and target invariants independent of persistence.
use crate::{
	Conversation, Error, Result, Task, Thread,
	federation::Delegation,
	registry::{EntityRef, Entry},
};
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Serialize, JsonSchema)]
pub struct ConversationResponse {
	pub conversation: Conversation,
	pub thread: Thread,
	pub task: Task,
	pub delegation: Delegation,
}

/// What opening a Thread returns (#1008, ADR 0140): the Thread's own fields
/// and, when a person asked an Agent or cluster for something, the
/// conversation record, the Root Task whose description is the request, and
/// its delegation (all `null` for a Thread opened without a request).
#[derive(Serialize, JsonSchema)]
pub struct ThreadOpened {
	#[serde(flatten)]
	pub thread: Thread,
	pub conversation: Option<Conversation>,
	pub task: Option<Task>,
	pub delegation: Option<Delegation>,
}
impl From<ConversationResponse> for ThreadOpened {
	fn from(value: ConversationResponse) -> Self {
		Self {
			thread: value.thread,
			conversation: Some(value.conversation),
			task: Some(value.task),
			delegation: Some(value.delegation),
		}
	}
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
	/// Tasks this page scanned and could not confirm because a Node did not answer.
	pub unconfirmed: Vec<crate::unconfirmed::Unconfirmed>,
}
