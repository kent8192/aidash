//! Generated agent request contracts remain independent of database rows.
use crate::{federation::Delegation, registry::Entry};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "GenerationRequest")]
pub struct Request {
	pub id: Uuid,
	pub tenant: String,
	pub policy_id: String,
	pub policy_revision: i64,
	pub task_id: Uuid,
	pub home_node: String,
	#[serde(skip_serializing)]
	pub foreign_intent: Option<Value>,
	pub prepared: bool,
	pub grant_id: Option<Uuid>,
	pub admission_id: Option<Uuid>,
	pub workspace_id: Uuid,
	#[serde(skip_serializing)]
	pub credential_id: Uuid,
	pub root_subject: String,
	pub subject_chain: Vec<String>,
	pub agent_id: String,
	pub agent_version: String,

	#[schemars(with = "Entry")]
	pub definition: Value,
	pub status: String,
	pub reason: String,
	pub depth: i32,
	pub token_limit: i64,
	pub quota_released: bool,
	pub expires_at: DateTime<Utc>,
	pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[schemars(rename = "GenerationAssignment")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Assignment {
	Existing { delegation: Delegation },
	Generated { generation: Box<Request> },
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[schemars(rename = "GenerationAction")]
#[serde(rename_all = "snake_case")]
pub enum Action {
	Approve,
	Deny,
	Stop,
	Delete,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[schemars(rename = "GenerationControl")]
#[serde(deny_unknown_fields)]
pub struct Control {
	pub action: Action,
	pub reason: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[schemars(rename = "GenerationHistory")]
pub struct History {
	pub sequence: i64,
	pub request_id: Uuid,
	pub status: String,
	pub actor: String,
	pub reason: String,
	pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[schemars(rename = "GenerationUsage")]
pub struct Usage {
	pub token_limit: i64,
	pub used_tokens: i64,
	pub inference_attempts: i64,
	pub compaction_call_limit: i64,
	pub compaction_calls: i64,
	pub embedding_calls: i64,
	pub embedding_call_limit: i64,
	pub summary_calls: i64,
	pub summary_call_limit: i64,
}

impl Request {
	pub fn resource_attributes(&self) -> Value {
		json!({"policy_id":self.policy_id,"root_subject":self.root_subject,"task_id":self.task_id})
	}
	pub fn active(&self) -> bool {
		matches!(
			self.status.as_str(),
			"PENDING_APPROVAL" | "QUEUED" | "ACTIVE"
		)
	}
}
impl Action {
	pub fn status(&self) -> &'static str {
		match self {
			Self::Approve => "QUEUED",
			Self::Deny => "DENIED",
			Self::Stop => "STOPPED",
			Self::Delete => "DELETED",
		}
	}
	pub fn authorization(&self) -> &'static str {
		match self {
			Self::Approve | Self::Deny => "generation.approve",
			Self::Stop => "generation.stop",
			Self::Delete => "generation.delete",
		}
	}
	pub fn accepts(&self, job: &Request) -> bool {
		match self {
			Self::Approve | Self::Deny => job.status == "PENDING_APPROVAL",
			Self::Stop => job.active(),
			Self::Delete => !job.active(),
		}
	}
}
