use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskStatus {
	Open,
	Claimed,
	Running,
	Completed,
	Failed,
	Blocked,
	Cancelled,
	Abandoned,
}

impl TaskStatus {
	pub fn can_transition(&self, next: &Self) -> bool {
		matches!(
			(self, next),
			(Self::Open, Self::Claimed | Self::Failed)
				| (Self::Claimed, Self::Running | Self::Failed)
				| (
					Self::Running,
					Self::Completed | Self::Failed | Self::Blocked
				) | (Self::Blocked, Self::Running)
				| (Self::Failed, Self::Open)
		) || (*next == Self::Cancelled
			&& !matches!(self, Self::Completed | Self::Cancelled | Self::Abandoned))
	}
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RunPhase {
	Ready,
	Thinking,
	ToolCall,
	Waiting,
	Completed,
	Failed,
	Cancelled,
}

pub use crate::run_state::*;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RunControl {
	Active,
	Paused,
	Cancelled,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunControlAction {
	Pause,
	Resume,
	Cancel,
}
impl RunControlAction {
	pub fn control(self) -> RunControl {
		match self {
			Self::Pause => RunControl::Paused,
			Self::Resume => RunControl::Active,
			Self::Cancel => RunControl::Cancelled,
		}
	}
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Pause => "pause",
			Self::Resume => "resume",
			Self::Cancel => "cancel",
		}
	}
}
macro_rules! text_enum {
	($name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
		impl $name {
			pub fn as_str(self) -> &'static str {
				match self { $(Self::$variant => $text),+ }
			}
		}
		impl std::fmt::Display for $name {
			fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
				f.write_str(self.as_str())
			}
		}
		impl TryFrom<String> for $name {
			type Error = crate::Error;
			fn try_from(value: String) -> crate::Result<Self> {
				serde_json::from_value(serde_json::Value::String(value)).map_err(Into::into)
			}
		}
	};
}
text_enum!(TaskStatus {
	Open => "OPEN",
	Claimed => "CLAIMED",
	Running => "RUNNING",
	Completed => "COMPLETED",
	Failed => "FAILED",
	Blocked => "BLOCKED",
	Cancelled => "CANCELLED",
	Abandoned => "ABANDONED",
});
text_enum!(RunPhase {
	Ready => "READY",
	Thinking => "THINKING",
	ToolCall => "TOOL_CALL",
	Waiting => "WAITING",
	Completed => "COMPLETED",
	Failed => "FAILED",
	Cancelled => "CANCELLED",
});
text_enum!(RunControl {
	Active => "ACTIVE",
	Paused => "PAUSED",
	Cancelled => "CANCELLED",
});
impl RunPhase {
	pub fn is_terminal(self) -> bool {
		matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
	}
}
impl TaskStatus {
	pub fn is_terminal(self) -> bool {
		matches!(
			self,
			Self::Completed | Self::Failed | Self::Cancelled | Self::Abandoned
		)
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Workspace {
	pub id: Uuid,
	pub title: String,
	pub goal: String,

	pub state: Value,
	pub revision: i64,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Task {
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub title: String,
	pub description: String,
	pub status: TaskStatus,

	pub requirements: Value,
	pub owner: Option<String>,
	pub created_by: String,
	pub dependencies: Vec<Uuid>,
	pub parent_id: Option<Uuid>,
	pub revision: i64,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct ChildTaskSummary {
	pub has_pending: bool,
	pub has_failed: bool,
}

impl ChildTaskSummary {
	pub fn include_status(&mut self, status: TaskStatus) {
		self.has_pending |= !matches!(status, TaskStatus::Completed | TaskStatus::Abandoned);
		self.has_failed |= matches!(
			status,
			TaskStatus::Failed | TaskStatus::Blocked | TaskStatus::Cancelled
		);
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewTask {
	pub title: String,
	pub description: String,
	#[serde(default = "empty_object")]
	pub requirements: Value,
	#[serde(default)]
	pub dependencies: Vec<Uuid>,
	pub parent_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Artifact {
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub task_id: Uuid,
	pub kind: String,
	pub name: String,
	pub content: Value,
	pub created_by: String,
	pub idempotency_key: String,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ArtifactInput {
	pub kind: String,
	pub name: String,
	pub content: Value,
}

impl ArtifactInput {
	pub fn validate(&self) -> Result<()> {
		nonempty(&self.name, "artifact name")?;
		if !matches!(
			self.kind.as_str(),
			"text" | "json" | "file_reference" | "code" | "structured_result"
		) {
			return Err(Error::Invalid("unsupported artifact kind".into()));
		}
		if matches!(self.kind.as_str(), "text" | "code" | "file_reference")
			&& !self.content.is_string()
		{
			return Err(Error::Invalid("artifact content must be a string".into()));
		}
		Ok(())
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Event {
	pub sequence: i64,
	pub id: Uuid,
	pub node_id: String,
	pub workspace_id: Option<Uuid>,
	pub kind: String,
	pub data: Value,
	pub created_at: DateTime<Utc>,
}

impl Event {
	pub fn cloud_event(&self) -> Value {
		json!({"specversion":"1.0", "id":self.id, "source":self.node_id, "type":self.kind,
            "subject":self.workspace_id, "time":self.created_at, "datacontenttype":"application/json", "data":self.data, "sequence":self.sequence})
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Conversation {
	pub created_by: String,
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub target: String,
	pub target_kind: String,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HumanRequest {
	pub answered_by: Option<String>,
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub run_id: Uuid,
	pub kind: String,
	pub prompt: String,
	pub response: Option<Value>,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Run {
	pub id: Uuid,
	pub task_id: Uuid,
	pub workspace_id: Uuid,
	pub home_node: String,
	pub agent_id: String,
	pub agent_version: String,
	pub state_version: StateVersion,
	pub state: RunState,
	pub recovery: RecoveryState,
	pub control: RunControl,
	pub context: crate::context::Context,
	pub step: i32,
	pub revision: i64,
	pub observed_input_seq: i64,
	pub ledger_worker_ready: bool,
	pub error: Option<String>,
	pub lease_owner: Option<Uuid>,
	pub lease_until: Option<DateTime<Utc>>,
	pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceSnapshot {
	pub workspace: Workspace,
	pub tasks: Vec<Task>,
	pub artifacts: Vec<Artifact>,
	pub events: Vec<Event>,
	pub messages: Vec<Message>,
}

/// A keyset page of one workspace collection. Pages stay below the peer
/// response limit even when the complete workspace is much larger.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SnapshotPage {
	pub items: Vec<Value>,
	pub next: Option<Uuid>,
}

pub fn empty_object() -> Value {
	json!({})
}
pub fn nonempty(s: &str, name: &str) -> Result<()> {
	if s.trim().is_empty() || s.len() > 64_000 {
		Err(Error::Invalid(format!(
			"{name} must contain 1 to 64000 bytes"
		)))
	} else {
		Ok(())
	}
}
pub fn qualified_agent(node: &str, id: &str, version: &str) -> String {
	format!("{node}/agents/{id}@{version}")
}

#[cfg(test)]
mod tests {
	use super::*;
	#[rstest::rstest]
	fn task_transitions_protect_terminal_states() {
		assert!(TaskStatus::Open.can_transition(&TaskStatus::Claimed));
		assert!(!TaskStatus::Open.can_transition(&TaskStatus::Completed));
		assert!(!TaskStatus::Completed.can_transition(&TaskStatus::Cancelled));
		assert!(TaskStatus::Running.can_transition(&TaskStatus::Blocked));
	}

	#[rstest::rstest]
	fn unsupported_wire_format_is_rejected() {
		assert!(
			serde_json::from_value::<Run>(json!({"phase":"THINKING","pending":{},"context":{}}))
				.is_err()
		);
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Message {
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub sender: String,
	pub content: String,
	pub idempotency_key: Option<String>,
	pub created_at: DateTime<Utc>,
}

// A required JSON content field may contain null, but may not be absent.
// deserialize_with prevents serde's missing-field adapter from supplying null.
pub fn required_json<'de, D: serde::Deserializer<'de>>(
	deserializer: D,
) -> std::result::Result<Value, D::Error> {
	Value::deserialize(deserializer)
}
