//! Receiver bindings, current authority checks and execution projections are independent of storage.
use super::Description;
use crate::{
	RunMetadata,
	identity::execution::ExecutionPrincipal,
	registry::{EntityRef, Search},
	semantic::Failure,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
mod semantic_failure;

/// First entry into a receiving Working Area behind one or more awake Runs.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunQueued {
	pub task_id: Uuid,
	pub run_id: Uuid,
	pub runs_ahead: usize,
	pub queued_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RemoteExecutionPhase {
	Admitted,
	Ready,
	Thinking,
	ToolCall,
	Waiting,
	Completed,
	Failed,
	Cancelled,
}
impl From<crate::RunPhase> for RemoteExecutionPhase {
	fn from(p: crate::RunPhase) -> Self {
		use crate::RunPhase as P;
		match p {
			P::Ready => Self::Ready,
			P::Thinking => Self::Thinking,
			P::ToolCall => Self::ToolCall,
			P::Waiting => Self::Waiting,
			P::Completed => Self::Completed,
			P::Failed => Self::Failed,
			P::Cancelled => Self::Cancelled,
		}
	}
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RemoteExecutionControlState {
	Inactive,
	Active,
	Paused,
	Cancelled,
}
impl From<crate::RunControl> for RemoteExecutionControlState {
	fn from(c: crate::RunControl) -> Self {
		match c {
			crate::RunControl::Active => Self::Active,
			crate::RunControl::Paused => Self::Paused,
			crate::RunControl::Cancelled => Self::Cancelled,
		}
	}
}
#[derive(Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemoteExecutionControl {
	Pause,
	Resume,
	Cancel,
	ResumeGoal { message_id: Uuid },
}
impl RemoteExecutionControl {
	pub fn action(&self) -> crate::RunControlAction {
		match self {
			Self::Pause => crate::RunControlAction::Pause,
			Self::Resume | Self::ResumeGoal { .. } => crate::RunControlAction::Resume,
			Self::Cancel => crate::RunControlAction::Cancel,
		}
	}
}
/// A list answer may continue only the specific Authority Pause shown to its author.
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolAskTarget {
	pub call: String,
	pub key: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectInput {
	/// Sent while preparing a new grant. Live rechecks refresh the lists through its Description.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tool_request: Option<crate::federation::tool_lists::ToolList>,
	#[serde(default)]
	pub task_id: Option<Uuid>,
	pub agent: EntityRef,
	pub requirements: Search,
}
#[derive(Clone)]
pub struct Record {
	/// Permanent cancellation of the original admission, including before activation.
	pub cancelled: bool,
	pub id: Uuid,
	/// The receiving Node.
	pub node: crate::nodes::NodeName,
	pub source_node: String,
	pub grant_id: Uuid,
	pub task_id: Uuid,
	pub tenant: String,
	pub credential_id: Uuid,
	pub subject_chain: Vec<String>,
	pub description: Value,
}
impl Record {
	pub fn matches(
		&self,
		identity: &ExecutionPrincipal,
		subjects: &[String],
		description: &Description,
	) -> serde_json::Result<bool> {
		Ok(!self.cancelled
			&& self.source_node == description.source_node
			&& self.grant_id == description.grant_id
			&& self.task_id == description.task.id
			&& self.tenant == identity.tenant
			&& self.credential_id == identity.credential_id
			&& self.subject_chain == subjects
			&& pinned(self.description.clone()) == pinned(serde_json::to_value(description)?))
	}
	/// The stored Description carrying the Home's Federation Tool Request from `fresh`, a
	/// Description the Home has just described, when it differs from the stored one; `None` when
	/// it does not. The Request is the only part replaced: the rest stays as admitted, the
	/// expiry included. Background checks that hold no Home connection read the Request from
	/// the admission (ADR 0139, decision 12), so a Request the Home widened or narrowed reaches
	/// them once a worker boundary has described it.
	pub fn with_tool_request(&self, fresh: &Description) -> serde_json::Result<Option<Value>> {
		let mut stored = self.description.clone();
		let Some(fields) = stored.as_object_mut() else {
			return Ok(None);
		};
		let current = fields
			.get("authorization_details")
			.cloned()
			.unwrap_or_else(|| Value::Array(Vec::new()));
		let request = serde_json::to_value(&fresh.authorization_details)?;
		if current == request {
			return Ok(None);
		}
		// Stored as a Description serializes it: absent when empty.
		if fresh.authorization_details.is_empty() {
			fields.remove("authorization_details");
		} else {
			fields.insert("authorization_details".into(), request);
		}
		Ok(Some(stored))
	}
	pub fn matches_run(&self, run: &RunMetadata, description: &Description) -> bool {
		self.source_node == run.home_node
			&& self.task_id == run.task_id
			&& description.task.thread_id == run.thread_id
			&& description.inspection.agent.id == run.agent_id
			&& description.inspection.agent.version == run.agent_version
	}
	pub fn view(&self, description: &Description) -> Admission {
		Admission {
			id: self.id,
			source_node: self.source_node.clone(),
			grant_id: self.grant_id,
			task_id: self.task_id,
			expires_at: description.expires_at,
		}
	}
}
/// The part of a description an admission pins. The Home's Federation Tool Request is
/// read fresh on every describe and the Home renews the grant's expiry while the Run
/// lives (ADR 0139), so neither may end an admission by changing.
fn pinned(mut description: serde_json::Value) -> serde_json::Value {
	if let Some(fields) = description.as_object_mut() {
		fields.remove("authorization_details");
		fields.remove("expires_at");
	}
	description
}
#[derive(Serialize, Deserialize)]
pub struct Admission {
	pub id: Uuid,
	pub source_node: String,
	pub grant_id: Uuid,
	pub task_id: Uuid,
	pub expires_at: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Activation {
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub run_id: Uuid,
	pub phase: RemoteExecutionPhase,
	pub control: RemoteExecutionControlState,
	pub error: Option<String>,
	#[serde(flatten, with = "semantic_failure")]
	#[schemars(with = "semantic_failure::Wire")]
	pub semantic_reason: Option<Failure>,
	/// Typed context-recovery pause reason. Omitted when absent, so peers
	/// without the field keep their exact wire shape.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub context_reason: Option<crate::context::recovery::Failure>,
	/// The tool call a paused Run stopped on that its Home may decline: the key, the tool and the
	/// kind of refusal, never the action, the subject or the receiver's own sentence (#763).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub declinable_call: Option<crate::run_state::DeclinableCall>,
}
impl Activation {
	/// Whether the remote Run can still continue: neither cancelled nor finished.
	/// A semantic cause stays reportable afterwards, but no recovery applies.
	pub fn recoverable(&self) -> bool {
		!matches!(self.control, RemoteExecutionControlState::Cancelled)
			&& !matches!(
				self.phase,
				RemoteExecutionPhase::Completed
					| RemoteExecutionPhase::Failed
					| RemoteExecutionPhase::Cancelled
			)
	}
}
/// What the receiver answers when its Home declines a call: the Run as `control` answers it,
/// and what was declined, so the Home can write its own record after the receiver has forgotten
/// the pause. The tool and the kind of refusal, never the action, the subject or a person.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Declined {
	pub activation: Activation,
	pub tool: String,
	pub kind: crate::tool::RefusalKind,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
	pub id: Uuid,
	pub content: String,
}
#[derive(Serialize, Deserialize)]
pub struct MessageReceipt {
	pub id: Uuid,
	pub run_id: Uuid,
	pub accepted: bool,
}
/// Whether a receiver still serves its Home's request about an admitted Run
/// (a Live preview, #1013), the admission being open: never for an ended or
/// cancelled Run; for a Run that never started (step 0, `READY`), only within
/// the grant's pinned lifetime. Once the Run has started, whether its grant
/// still lives is the Home's to decide, and the Home has just asked.
pub fn serves_home(
	phase: crate::RunPhase,
	control: crate::RunControl,
	step: i32,
	expires_at: DateTime<Utc>,
	now: DateTime<Utc>,
) -> bool {
	if phase.is_terminal() || control == crate::RunControl::Cancelled {
		return false;
	}
	let started = step > 0 || phase != crate::RunPhase::Ready;
	started || expires_at > now
}
#[cfg(test)]
mod tests;
