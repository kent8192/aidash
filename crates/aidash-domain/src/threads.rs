//! Collaboration contracts and target invariants independent of persistence.
use crate::{Conversation, Task, Thread, federation::Delegation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadTargetKind {
	Agent,
	Node,
}

impl ThreadTargetKind {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Agent => "agent",
			Self::Node => "node",
		}
	}

	pub(crate) fn deserialize_name<'de, D: serde::Deserializer<'de>>(
		deserializer: D,
	) -> std::result::Result<String, D::Error> {
		Ok(Self::deserialize(deserializer)?.as_str().into())
	}
}

#[derive(Serialize, JsonSchema)]
pub struct ConversationResponse {
	pub conversation: Conversation,
	pub thread: Thread,
	pub task: Task,
	pub delegation: Option<Delegation>,
}

/// What opening a Thread returns (#1008, ADR 0140): the Thread's own fields
/// and, when a person asked an Agent or Node for something, the
/// conversation record, the Root Task whose description is the request, and
/// its delegation when an Agent was selected.
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
			delegation: value.delegation,
		}
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
