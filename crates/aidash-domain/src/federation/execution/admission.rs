//! Receiver bindings, current authority checks and execution projections are independent of storage.
use super::Description;
use crate::{
	RunMetadata,
	identity::execution::ExecutionPrincipal,
	registry::{AgentConfig, EntityRef, Search},
	semantic::Failure,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
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
}
impl RemoteExecutionControl {
	pub fn action(&self) -> crate::RunControlAction {
		match self {
			Self::Pause => crate::RunControlAction::Pause,
			Self::Resume => crate::RunControlAction::Resume,
			Self::Cancel => crate::RunControlAction::Cancel,
		}
	}
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectInput {
	#[serde(default)]
	pub task_id: Option<Uuid>,
	pub tenant: String,
	pub subject: String,
	pub agent: EntityRef,
	pub requirements: Search,
	#[serde(default)]
	pub compactor: Option<EntityRef>,
}
#[derive(Clone)]
pub struct Record {
	pub id: Uuid,
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
		Ok(self.source_node == description.source_node
			&& self.grant_id == description.grant_id
			&& self.task_id == description.task.id
			&& self.tenant == identity.tenant
			&& self.credential_id == identity.credential_id
			&& self.subject_chain == subjects
			&& self.description == serde_json::to_value(description)?)
	}
	pub fn matches_run(&self, run: &RunMetadata, description: &Description) -> bool {
		self.source_node == run.home_node
			&& self.task_id == run.task_id
			&& description.task.workspace_id == run.workspace_id
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
#[derive(Serialize, Deserialize)]
pub struct Admission {
	pub id: Uuid,
	pub source_node: String,
	pub grant_id: Uuid,
	pub task_id: Uuid,
	pub expires_at: DateTime<Utc>,
}
#[derive(Serialize, Deserialize)]
pub struct Activation {
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub run_id: Uuid,
	pub phase: RemoteExecutionPhase,
	pub control: RemoteExecutionControlState,
	pub error: Option<String>,
	#[serde(default)]
	pub semantic_reason: Option<Failure>,
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
pub fn require_workspace_agent(agent: &AgentConfig) -> crate::Result<()> {
	if agent.core_capabilities.enabled() {
		return Err(crate::Error::Invalid("remote execution requires a workspace Agent without local core working-area capabilities; transfer files into an explicitly admitted local thread".into()));
	};
	Ok(())
}
#[cfg(test)]
mod tests;
