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
/// The kind of actor that last changed a Run's control. It is shown to anyone
/// who can see the Run; the person and the reason are not.
#[derive(
	Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ControlSource {
	/// A Tenant Subject, or the Run's creation.
	#[default]
	Tenant,
	/// A Deployment Administrator's Safety Stop.
	DeploymentAdministrator,
	/// Aidash itself, such as the pause for an identity status outage.
	System,
}
impl ControlSource {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Tenant => "tenant",
			Self::DeploymentAdministrator => "deployment_administrator",
			Self::System => "system",
		}
	}
}
impl std::fmt::Display for ControlSource {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(self.as_str())
	}
}
impl TryFrom<String> for ControlSource {
	type Error = crate::Error;
	fn try_from(value: String) -> crate::Result<Self> {
		serde_json::from_value(serde_json::Value::String(value)).map_err(Into::into)
	}
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
	/// The Node of its Tenant that the Workspace, its Tasks and its Runs belong to.
	pub node: crate::nodes::NodeName,
	pub title: String,
	pub goal: String,

	pub state: Value,
	pub revision: i64,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
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
	/// The Run that published this Artifact on this Node. Only the Run's own
	/// publication writes it; Artifacts that people or other Nodes deliver have
	/// none. Source citations in its content resolve only against this Run.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub run_id: Option<Uuid>,
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
	/// The Node of the Workspace or executing Run; none for a Tenant-level event.
	pub node: Option<crate::nodes::NodeName>,
	pub workspace_id: Option<Uuid>,
	pub kind: String,
	pub data: Value,
	pub created_at: DateTime<Utc>,
}

impl Event {
	/// The CloudEvent `source` is the Node address, or the Deployment for a
	/// Tenant-level event that belongs to no Node.
	pub fn source(&self, deployment: &str) -> String {
		self.node.as_ref().map_or_else(
			|| deployment.to_owned(),
			|node| format!("{deployment}/{node}"),
		)
	}

	pub fn cloud_event(&self, deployment: &str) -> Value {
		json!({"specversion":"1.0", "id":self.id, "source":self.source(deployment), "type":self.kind,
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
	/// When the Run waiting for this question continues without an answer, if
	/// its person set an Answer Deadline. State and Human Request listings fill
	/// it from the Run's waiting state.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub answer_by: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Run {
	pub id: Uuid,
	pub task_id: Uuid,
	pub workspace_id: Uuid,
	/// The Node that executes the Run.
	pub node: crate::nodes::NodeName,
	/// The address of the Node that owns the Run's Task; the executing Node's
	/// own address for a local Run.
	pub home_node: String,
	pub agent_id: String,
	pub agent_version: String,
	pub state_version: StateVersion,
	pub state: RunState,
	pub recovery: RecoveryState,
	pub control: RunControl,
	/// Counts control decisions only; worker progress does not change it.
	#[serde(default)]
	pub control_revision: i64,
	/// The kind of actor that last changed `control`.
	#[serde(default)]
	pub control_source: ControlSource,
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
/// The subject of an agent version approved on `node`.
pub fn qualified_agent(node: &crate::nodes::NodeAddress, id: &str, version: &str) -> String {
	agent_subject(&node.to_string(), id, version)
}

/// `qualified_agent` for a Node address that is held as text.
pub fn agent_subject(node: &str, id: &str, version: &str) -> String {
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
	/// The Run that published this message on this Node. Only the Run's own
	/// publication writes it; messages that people or other Nodes deliver have
	/// none. Source citations in its content resolve only against this Run.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub run_id: Option<Uuid>,
}

// A required JSON content field may contain null, but may not be absent.
// deserialize_with prevents serde's missing-field adapter from supplying null.
pub fn required_json<'de, D: serde::Deserializer<'de>>(
	deserializer: D,
) -> std::result::Result<Value, D::Error> {
	Value::deserialize(deserializer)
}
