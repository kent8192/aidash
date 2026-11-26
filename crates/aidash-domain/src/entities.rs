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
pub struct Thread {
	pub id: Uuid,
	pub scratch_space_id: Uuid,
	/// The Node of its Tenant that the Thread, its Tasks and its Runs belong to.
	pub node: crate::nodes::NodeName,
	pub title: String,
	/// The person (or the Duty's requester) who opened it.
	pub created_by: String,
	pub revision: i64,
	pub created_at: DateTime<Utc>,
}

/// Structured admission refusals shared by local starts and federation.
#[derive(
	Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, thiserror::Error,
)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum TaskModelRefusal {
	#[error("This agent needs a Task Model")]
	TaskModelRequired,
	#[error("The selected model is not registered")]
	TaskModelNotRegistered,
	#[error("The selected model needs Catalog Approval")]
	TaskModelNotApproved {
		model: crate::registry::EntityRef,
		can_approve: bool,
	},
	#[error("The selected model cannot be used: {reasons:?}")]
	TaskModelUnusable { reasons: Vec<String> },
}

/// The exact model and optional reasoning effort selected for a Task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskModel {
	pub model: crate::registry::EntityRef,
	#[serde(default)]
	#[schemars(schema_with = "task_reasoning_effort_schema")]
	pub reasoning_effort: Option<crate::model::ReasoningEffort>,
}

// Deserialize the provider's full enum so admission can return a structured
// refusal, while generated clients offer only levels a Task can actually select.
fn task_reasoning_effort_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
	schemars::json_schema!({
		"type": ["string", "null"],
		"enum": ["low", "medium", "high", "xhigh", "max", null]
	})
}

impl TaskModel {
	pub fn validate(&self) -> crate::Result<()> {
		use crate::model::ReasoningEffort::{Minimal, None};
		if matches!(self.reasoning_effort, Some(None | Minimal)) {
			return Err(crate::Error::Invalid(
				"Task Model reasoning effort must be at least low".into(),
			));
		}
		Ok(())
	}
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Task {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub task_model: Option<TaskModel>,
	pub id: Uuid,
	pub thread_id: Uuid,
	pub title: String,
	pub description: String,
	pub status: TaskStatus,

	pub requirements: Value,
	pub owner: Option<String>,
	pub created_by: String,
	pub dependencies: Vec<Uuid>,
	pub parent_id: Option<Uuid>,
	/// The Duty that started this Task automatically, if any.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub started_by_duty: Option<Uuid>,
	pub revision: i64,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct ChildTaskSummary {
	pub has_pending: bool,
	pub has_failed: bool,
	pub open_tasks: u64,
	/// The open children nobody is working on or can start (#772).
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub unassigned: Vec<crate::unassigned::UnassignedTask>,
	#[serde(with = "fingerprint_hex")]
	#[schemars(with = "String")]
	pub terminal_fingerprint: [u8; 16],
}

mod fingerprint_hex {
	use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

	pub fn serialize<S: Serializer>(value: &[u8; 16], serializer: S) -> Result<S::Ok, S::Error> {
		let encoded: String = value.iter().map(|byte| format!("{byte:02x}")).collect();
		serializer.serialize_str(&encoded)
	}

	pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 16], D::Error> {
		let text = String::deserialize(deserializer)?;
		if text.len() != 32
			|| !text
				.bytes()
				.all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
		{
			return Err(D::Error::custom(
				"expected a 32-character lowercase hexadecimal fingerprint",
			));
		}
		let mut value = [0; 16];
		for (index, byte) in value.iter_mut().enumerate() {
			*byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
				.map_err(D::Error::custom)?;
		}
		Ok(value)
	}
}

impl ChildTaskSummary {
	pub fn fingerprint(&self) -> String {
		self.terminal_fingerprint
			.iter()
			.map(|byte| format!("{byte:02x}"))
			.collect()
	}
	pub fn include_status(&mut self, status: TaskStatus) {
		let pending = !matches!(status, TaskStatus::Completed | TaskStatus::Abandoned);
		self.has_pending |= pending;
		self.open_tasks += u64::from(pending);
		self.has_failed |= matches!(
			status,
			TaskStatus::Failed | TaskStatus::Blocked | TaskStatus::Cancelled
		);
	}
	pub fn include_task(&mut self, id: Uuid, status: TaskStatus, revision: i64) {
		use sha2::{Digest, Sha256};
		self.include_status(status);
		if matches!(
			status,
			TaskStatus::Completed
				| TaskStatus::Failed
				| TaskStatus::Cancelled
				| TaskStatus::Abandoned
				| TaskStatus::Blocked
		) {
			let mut digest = Sha256::new();
			digest.update(id.as_bytes());
			digest.update(status.as_str().as_bytes());
			digest.update(revision.to_be_bytes());
			for (slot, byte) in self.terminal_fingerprint.iter_mut().zip(digest.finalize()) {
				*slot ^= byte;
			}
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewTask {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub task_model: Option<TaskModel>,
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
	pub thread_id: Uuid,
	pub task_id: Uuid,
	pub kind: String,
	pub name: String,
	pub content: Value,
	pub created_by: String,
	pub idempotency_key: String,
	pub created_at: DateTime<Utc>,
	/// The Run that published this Artifact on this Node. Only the Run's own
	/// publication writes it; Artifacts that people or other Nodes deliver have
	/// none.
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
	/// The Node of the Thread or executing Run; none for a Tenant-level event.
	pub node: Option<crate::nodes::NodeName>,
	pub thread_id: Option<Uuid>,
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
            "subject":self.thread_id, "time":self.created_at, "datacontenttype":"application/json", "data":self.data, "sequence":self.sequence})
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Conversation {
	/// Fixed when the conversation starts.
	pub scratch_space_id: Uuid,
	pub created_by: String,
	pub id: Uuid,
	pub thread_id: Uuid,
	pub target: String,
	pub target_kind: String,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HumanRequest {
	pub answered_by: Option<String>,
	pub id: Uuid,
	pub thread_id: Uuid,
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
	pub thread_id: Uuid,
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
pub struct ThreadSnapshot {
	pub thread: Thread,
	pub tasks: Vec<Task>,
	pub artifacts: Vec<Artifact>,
	pub events: Vec<Event>,
	pub messages: Vec<Message>,
	/// Rows this read scanned and could not confirm because a Node did not
	/// answer (#770). Peers on an older build omit it.
	#[serde(default)]
	pub unconfirmed: Vec<crate::unconfirmed::Unconfirmed>,
}

/// A keyset page of one thread collection. Pages stay below the peer
/// response limit even when the complete thread is much larger.
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
	pub thread_id: Uuid,
	pub sender: String,
	pub content: String,
	pub idempotency_key: Option<String>,
	pub created_at: DateTime<Utc>,
	/// The Run that published this message on this Node. Only the Run's own
	/// publication writes it; messages that people or other Nodes deliver have
	/// none.
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

#[cfg(test)]
mod task_model_tests {
	use super::*;
	#[test]
	fn task_model_effort_range_includes_unspecified_and_excludes_none_and_minimal() {
		use crate::model::ReasoningEffort::*;
		for (effort, valid) in [
			(Option::None, true),
			(Some(None), false),
			(Some(Minimal), false),
			(Some(Low), true),
			(Some(Medium), true),
			(Some(High), true),
			(Some(Xhigh), true),
			(Some(Max), true),
		] {
			let choice = TaskModel {
				model: crate::registry::EntityRef {
					id: "reasoner".into(),
					version: "1.0.0".into(),
				},
				reasoning_effort: effort,
			};
			assert_eq!(choice.validate().is_ok(), valid, "{choice:?}");
		}
	}
}
