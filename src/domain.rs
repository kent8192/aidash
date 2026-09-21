use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Workspace {
	pub id: Uuid,
	pub title: String,
	pub goal: String,
	#[schema(value_type = std::collections::BTreeMap<String, Value>)]
	pub state: Value,
	pub revision: i64,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Task {
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub title: String,
	pub description: String,
	pub status: String,
	#[schema(value_type = std::collections::BTreeMap<String, Value>)]
	pub requirements: Value,
	pub owner: Option<String>,
	pub created_by: String,
	pub dependencies: Vec<Uuid>,
	pub parent_id: Option<Uuid>,
	pub revision: i64,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewTask {
	pub title: String,
	pub description: String,
	#[serde(default = "empty_object")]
	#[schema(value_type = std::collections::BTreeMap<String, Value>)]
	pub requirements: Value,
	#[serde(default)]
	pub dependencies: Vec<Uuid>,
	pub parent_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Conversation {
	pub created_by: String,
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub target: String,
	pub target_kind: String,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Run {
	pub id: Uuid,
	pub task_id: Uuid,
	pub workspace_id: Uuid,
	pub home_node: String,
	pub agent_id: String,
	pub agent_version: String,
	pub phase: String,
	pub control: String,
	#[schema(value_type = crate::context::Context)]
	pub context: Value,
	pub pending: Value,
	pub step: i32,
	pub revision: i64,
	pub error: Option<String>,
	pub lease_owner: Option<Uuid>,
	pub lease_until: Option<DateTime<Utc>>,
	pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct WorkspaceSnapshot {
	pub workspace: Workspace,
	pub tasks: Vec<Task>,
	pub artifacts: Vec<Artifact>,
	pub events: Vec<Event>,
	pub messages: Vec<Message>,
}

/// A keyset page of one workspace collection. Pages stay below the peer
/// response limit even when the complete workspace is much larger.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
	#[test]
	fn task_transitions_protect_terminal_states() {
		assert!(TaskStatus::Open.can_transition(&TaskStatus::Claimed));
		assert!(!TaskStatus::Open.can_transition(&TaskStatus::Completed));
		assert!(!TaskStatus::Completed.can_transition(&TaskStatus::Cancelled));
		assert!(TaskStatus::Running.can_transition(&TaskStatus::Blocked));
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Message {
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub sender: String,
	pub content: String,
	pub idempotency_key: Option<String>,
	pub created_at: DateTime<Utc>,
}
