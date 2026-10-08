//! Current execution format. Only the storage codec handles unvalidated JSON.
use super::{Run, RunPhase};
use crate::{
	Error, Result,
	media::Selection,
	provider::{ModelResponse, ToolCall},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
mod row;
#[cfg(test)]
mod tests;
pub use row::RawRun;
pub use row::{RunInspection, RunMetadata};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, schemars::JsonSchema)]

pub struct StateVersion(u32);
impl Default for StateVersion {
	fn default() -> Self {
		Self(1)
	}
}
impl<'de> Deserialize<'de> for StateVersion {
	fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
		match u32::deserialize(d)? {
			1 => Ok(Self(1)),
			_ => Err(serde::de::Error::custom("unsupported run state version")),
		}
	}
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadyState {}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThinkingState {
	pub force_workspace_read_compaction: bool,
	pub deferred_workspace_read: Option<Box<DeferredRead>>,
	pub deferred_skill_read: Option<Box<DeferredRead>>,
	pub deferred_workspace_observation: Option<Box<DeferredRead>>,
	pub selected_media: Vec<Selection>,
	pub media_intake_through_seq: Option<i64>,
	pub deferred_run_message_reads: Vec<Uuid>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeferredRead {
	pub message: String,
	pub call: ToolCall,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadPlan {
	pub step: i32,
	pub cursor: usize,
	pub call: ToolCall,
	#[serde(deserialize_with = "super::required_json")]
	pub result: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalDecision {
	pub key: String,
	pub call: ToolCall,
	pub approved: bool,
	pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolCallState {
	pub response: ModelResponse,
	pub response_epoch: i64,
	pub cursor: usize,
	pub included_input_seq: i64,
	pub request_window: usize,
	pub request_tokens: usize,
	pub finalizing: bool,
	pub media_inferred_seq_before_response: i64,
	pub observed_input_seq_before_response: i64,
	pub inferred_selected_media: Vec<Selection>,
	pub deferred_selected_media: Vec<Selection>,
	pub selected_media: Vec<Selection>,
	pub deferred_human_media: bool,
	pub media_inferred_through_seq: Option<i64>,
	pub media_intake_through_seq: i64,
	pub required_run_message_reads: Vec<Uuid>,
	pub references_read_at_inference: bool,
	pub run_message_catchup: bool,
	pub run_message_summary_end_seq: i64,
	pub run_message_summary_limit: usize,
	pub deferred_reads: ThinkingState,
	pub workspace_read_plan: Option<ReadPlan>,
	pub skill_read_plan: Option<ReadPlan>,
	pub workspace_observation_plan: Option<ReadPlan>,
	pub workbench_approval_result: Option<ApprovalDecision>,
}
impl ToolCallState {
	pub fn selected_media(&self) -> Vec<Selection> {
		self.deferred_selected_media
			.iter()
			.chain(&self.selected_media)
			.cloned()
			.collect()
	}
	pub fn stale(
		&self,
		context: &mut crate::context::Context,
		observed: &mut i64,
	) -> ThinkingState {
		context.media_inferred_seq = self.media_inferred_seq_before_response;
		*observed = self.observed_input_seq_before_response;
		let mut selected_media = Vec::new();
		for value in self
			.inferred_selected_media
			.iter()
			.chain(&self.deferred_selected_media)
			.chain(&self.selected_media)
		{
			if !selected_media.iter().any(|s: &Selection| {
				s.file_id == value.file_id && s.expected_digest == value.expected_digest
			}) {
				selected_media.push(value.clone());
			}
		}
		ThinkingState {
			selected_media,
			media_intake_through_seq: Some(self.media_intake_through_seq),
			deferred_run_message_reads: self.required_run_message_reads.clone(),
			..Default::default()
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
	tag = "phase",
	content = "data",
	rename_all = "SCREAMING_SNAKE_CASE",
	deny_unknown_fields
)]
pub enum ResumeState {
	Ready(ReadyState),
	Thinking(ThinkingState),
	ToolCall(Box<ToolCallState>),
}
impl ResumeState {
	pub fn into_state(self) -> RunState {
		match self {
			Self::Ready(s) => RunState::Ready(s),
			Self::Thinking(s) => RunState::Thinking(s),
			Self::ToolCall(s) => RunState::ToolCall(s),
		}
	}
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FailureTarget {
	Failed,
	Cancelled,
}
impl FailureTarget {
	pub fn task_status(self) -> super::TaskStatus {
		match self {
			Self::Failed => super::TaskStatus::Failed,
			Self::Cancelled => super::TaskStatus::Cancelled,
		}
	}
	pub fn into_state(self) -> RunState {
		match self {
			Self::Failed => RunState::Failed(TerminalState {}),
			Self::Cancelled => RunState::Cancelled(TerminalState {}),
		}
	}
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "reason", rename_all = "snake_case", deny_unknown_fields)]
pub enum WaitingState {
	Dependencies {
		wake_at: DateTime<Utc>,
		resume: ReadyState,
	},
	Children {
		wake_at: DateTime<Utc>,
		resume: ThinkingState,
	},
	Timer {
		wake_at: DateTime<Utc>,
		resume: ResumeState,
	},
	Human {
		request_id: Uuid,
		resume: ResumeState,
	},
	CoreApproval {
		approval_id: Uuid,
		resume: ThinkingState,
	},
	ExternalApproval {
		request_id: Uuid,
		key: String,
		call: ToolCall,
		expires_at: DateTime<Utc>,
		resume: Box<ToolCallState>,
	},
	Reconciliation {
		request_id: Uuid,
		key: String,
		resume: Box<ToolCallState>,
	},
	FailureDelivery {
		target: FailureTarget,
		wake_at: DateTime<Utc>,
		last_delivery_error: Option<String>,
	},
}
impl WaitingState {
	pub fn deadline(&self) -> Option<DateTime<Utc>> {
		match self {
			Self::Dependencies { wake_at, .. }
			| Self::Children { wake_at, .. }
			| Self::Timer { wake_at, .. }
			| Self::FailureDelivery { wake_at, .. } => Some(*wake_at),
			Self::ExternalApproval { expires_at, .. } => Some(*expires_at),
			_ => None,
		}
	}
	pub fn request_id(&self) -> Option<Uuid> {
		match self {
			Self::Human { request_id, .. }
			| Self::ExternalApproval { request_id, .. }
			| Self::Reconciliation { request_id, .. } => Some(*request_id),
			_ => None,
		}
	}
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerminalState {}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
	tag = "phase",
	content = "data",
	rename_all = "SCREAMING_SNAKE_CASE",
	deny_unknown_fields
)]
pub enum RunState {
	Ready(ReadyState),
	Thinking(ThinkingState),
	ToolCall(Box<ToolCallState>),
	Waiting(Box<WaitingState>),
	Completed(TerminalState),
	Failed(TerminalState),
	Cancelled(TerminalState),
}
impl Default for RunState {
	fn default() -> Self {
		Self::Ready(ReadyState {})
	}
}
impl RunState {
	pub fn phase(&self) -> RunPhase {
		match self {
			Self::Ready(_) => RunPhase::Ready,
			Self::Thinking(_) => RunPhase::Thinking,
			Self::ToolCall(_) => RunPhase::ToolCall,
			Self::Waiting(_) => RunPhase::Waiting,
			Self::Completed(_) => RunPhase::Completed,
			Self::Failed(_) => RunPhase::Failed,
			Self::Cancelled(_) => RunPhase::Cancelled,
		}
	}
	pub fn failure_delivery(&self) -> bool {
		matches!(self, Self::Waiting(s) if matches!(**s, WaitingState::FailureDelivery{..}))
	}
	pub fn tool(&self) -> Result<&ToolCallState> {
		match self {
			Self::ToolCall(s) => Ok(s),
			_ => Err(Error::Invalid("expected ToolCall state".into())),
		}
	}
	pub fn tool_mut(&mut self) -> Result<&mut ToolCallState> {
		match self {
			Self::ToolCall(s) => Ok(s),
			_ => Err(Error::Invalid("expected ToolCall state".into())),
		}
	}
	pub fn resume(self) -> Result<ResumeState> {
		match self {
			Self::Ready(s) => Ok(ResumeState::Ready(s)),
			Self::Thinking(s) => Ok(ResumeState::Thinking(s)),
			Self::ToolCall(s) => Ok(ResumeState::ToolCall(s)),
			_ => Err(Error::Invalid("state cannot be suspended".into())),
		}
	}
	pub fn validate(&self) -> Result<()> {
		let check = |s: &ToolCallState| {
			if s.cursor > s.response.tool_calls.len()
				|| s.response_epoch < 0
				|| s.included_input_seq < 0
			{
				Err(Error::Invalid("invalid ToolCall progress".into()))
			} else {
				Ok(())
			}
		};
		match self {
			Self::ToolCall(s) => check(s),
			Self::Waiting(s) => match s.as_ref() {
				WaitingState::ExternalApproval { resume, .. }
				| WaitingState::Reconciliation { resume, .. } => check(resume),
				WaitingState::Timer {
					resume: ResumeState::ToolCall(s),
					..
				}
				| WaitingState::Human {
					resume: ResumeState::ToolCall(s),
					..
				} => check(s),
				_ => Ok(()),
			},
			_ => Ok(()),
		}
	}
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecoveryState {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub semantic_reason: Option<crate::semantic::Failure>,
	pub retry: Option<RetryState>,
	pub lease_recovered: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetryState {
	pub count: u32,
	pub at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct PendingState {
	state_version: StateVersion,
	#[serde(deserialize_with = "super::required_json")]
	data: Value,
	recovery: RecoveryState,
}
pub fn encode(state: &RunState, recovery: &RecoveryState) -> Result<Value> {
	state.validate()?;
	let data = match state {
		RunState::Ready(s) => serde_json::to_value(s)?,
		RunState::Thinking(s) => serde_json::to_value(s)?,
		RunState::ToolCall(s) => serde_json::to_value(s)?,
		RunState::Waiting(s) => serde_json::to_value(s)?,
		RunState::Completed(s) | RunState::Failed(s) | RunState::Cancelled(s) => {
			serde_json::to_value(s)?
		}
	};
	Ok(serde_json::to_value(PendingState {
		state_version: StateVersion::default(),
		data,
		recovery: recovery.clone(),
	})?)
}
pub fn decode(phase: RunPhase, pending: Value) -> Result<(RunState, RecoveryState)> {
	let pending: PendingState = serde_json::from_value(pending)
		.map_err(|_| Error::Invalid("invalid pending envelope or state version".into()))?;
	let data = pending.data;
	let state = (|| -> std::result::Result<RunState, serde_json::Error> {
		Ok(match phase {
			RunPhase::Ready => RunState::Ready(serde_json::from_value(data)?),
			RunPhase::Thinking => RunState::Thinking(serde_json::from_value(data)?),
			RunPhase::ToolCall => RunState::ToolCall(serde_json::from_value(data)?),
			RunPhase::Waiting => RunState::Waiting(serde_json::from_value(data)?),
			RunPhase::Completed => RunState::Completed(serde_json::from_value(data)?),
			RunPhase::Failed => RunState::Failed(serde_json::from_value(data)?),
			RunPhase::Cancelled => RunState::Cancelled(serde_json::from_value(data)?),
		})
	})()
	.map_err(|_| Error::Invalid("invalid phase payload".into()))?;
	state.validate()?;
	Ok((state, pending.recovery))
}
impl Run {
	/// Native admission persists this context in the same transaction as Run
	/// insertion. An existing graph can only be reused byte-for-byte.
	pub fn bind(&mut self, snapshot: crate::registry::bindings::BindingSnapshot) -> Result<()> {
		snapshot.validate()?;
		if snapshot.remote != (snapshot.agent.registry_node != self.home_node)
			|| snapshot.agent.id != self.agent_id
			|| snapshot.agent.version != self.agent_version
		{
			return Err(Error::Invalid(
				"Binding snapshot belongs to a different Agent".into(),
			));
		}
		if let Some(saved) = &self.context.binding_snapshot {
			return if **saved == snapshot {
				Ok(())
			} else {
				Err(Error::Conflict("Run Bindings are immutable".into()))
			};
		}
		if self.step != 0 || !matches!(self.state, RunState::Ready(_)) {
			return Err(Error::Conflict(
				"Bindings must be admitted before Run activation".into(),
			));
		}
		self.context.binding_snapshot = Some(Box::new(snapshot));
		Ok(())
	}
	pub fn phase(&self) -> RunPhase {
		self.state.phase()
	}
	pub fn included_input_seq(&self) -> i64 {
		match &self.state {
			RunState::ToolCall(s) => s.included_input_seq,
			RunState::Waiting(wait) => match wait.as_ref() {
				WaitingState::ExternalApproval { resume, .. }
				| WaitingState::Reconciliation { resume, .. } => resume.included_input_seq,
				WaitingState::Human {
					resume: ResumeState::ToolCall(s),
					..
				}
				| WaitingState::Timer {
					resume: ResumeState::ToolCall(s),
					..
				} => s.included_input_seq,
				_ => self.observed_input_seq,
			},
			_ => self.observed_input_seq,
		}
	}
	pub fn stored_pending(&self) -> Result<Value> {
		encode(&self.state, &self.recovery)
	}
	pub fn suspend(&mut self, wait: impl FnOnce(ResumeState) -> WaitingState) -> Result<()> {
		let resume = self.state.clone().resume()?;
		self.state = RunState::Waiting(Box::new(wait(resume)));
		Ok(())
	}
}

impl Run {
	/// Receiver cleanup changes only the cancelled disposition and clears its prior execution error.
	pub fn cancelled_delivery(&self) -> Self {
		let mut cancelled = self.clone();
		cancelled.state = RunState::Cancelled(TerminalState {});
		cancelled.error = None;
		cancelled
	}
}

pub mod persistence;
