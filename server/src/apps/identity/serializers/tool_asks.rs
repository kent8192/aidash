//! Calls of remote Runs that wait for the people who hold a Federation tool list (ADR 0139).
use aidash_domain::federation::tool_lists::{AskChoice, AskSide};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One call that waits for an answer from the people of one side of a registration.
#[derive(Serialize, JsonSchema)]
pub(crate) struct ToolAskRecord {
	/// Whose people answer: `receiver` for a remote Run this Node hosts for the registered Home,
	/// `home` for a Remote Grant this Node issued to the registered receiving Node.
	pub(crate) side: AskSide,
	/// The path id of the answer: the admission of a receiver ask, the grant of a Home ask.
	pub(crate) id: Uuid,
	/// The remote Run.
	pub(crate) run_id: Uuid,
	/// The Home's Remote Grant.
	pub(crate) grant_id: Uuid,
	pub(crate) task_id: Uuid,
	pub(crate) task_title: Option<String>,
	/// The tool the Agent called.
	pub(crate) tool: String,
	/// The key the lists match, which an answer adds as an exact entry by default.
	pub(crate) key: String,
	/// When the Run stopped; absent on the Home, which learns the pause from the receiver.
	pub(crate) paused_since: Option<DateTime<Utc>>,
}

/// The calls waiting for an answer about one registration, receiver asks first.
#[derive(Serialize, JsonSchema)]
pub(crate) struct ToolAsks {
	pub(crate) asks: Vec<ToolAskRecord>,
}

/// How the people of a side answer one ask.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolAskAnswerInput {
	pub choice: AskChoice,
	/// A tool name or `*` pattern that matches the asked tool, added by `add`, `run_only` and
	/// `block` instead of the exact tool.
	#[serde(default)]
	pub pattern: Option<String>,
}

/// What an answer did.
#[derive(Serialize, JsonSchema)]
pub(crate) struct ToolAskAnswered {
	pub(crate) side: AskSide,
	pub(crate) choice: AskChoice,
	pub(crate) key: String,
	/// The entry the answer listed, for `run_only`, `add` and `block`.
	pub(crate) pattern: Option<String>,
	/// Whether the remote Run was resumed. It pauses again if the other side still lacks the tool.
	pub(crate) resumed: bool,
}
