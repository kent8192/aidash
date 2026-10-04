use serde::{Deserialize, Serialize};
// Serializable contracts for workbench.

use crate::apps::registry::workbench::models::AgentTestSession;
use chrono::DateTime;
use chrono::Utc;
use reinhardt::Validate;
use schemars::JsonSchema;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FixtureStatus {
	Success,
	Failure,
	Denied,
	Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
	pub status: FixtureStatus,
	pub response: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestInput {
	pub expected_revision: i64,
	pub message: String,
	#[serde(default = "simulated_mode")]
	pub mode: String,
	pub profile_id: Option<String>,
	pub continue_from: Option<Uuid>,
	#[serde(default)]
	pub fixtures: BTreeMap<String, Fixture>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, JsonSchema)]
pub struct TestSession {
	pub id: Uuid,
	pub draft_id: Uuid,
	pub tenant: String,
	pub revision: i64,
	pub status: String,
	pub scenario: Value,
	pub conversation: Option<Value>,
	pub tool_calls: Option<Value>,
	pub usage: Value,
	pub error: Option<String>,
	pub created_at: DateTime<Utc>,
	pub updated_at: DateTime<Utc>,
	pub expires_at: DateTime<Utc>,
	pub expired_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, sqlx::FromRow, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct TestLimits {
	pub tenant: String,
	#[validate(range(min = 1024, max = 1000000))]
	pub max_input_bytes: i32,
	#[validate(range(min = 64, max = 8192))]
	pub max_output_tokens: i32,
	#[validate(range(min = 256, max = 1000000))]
	pub max_total_tokens: i32,
	#[validate(range(min = 1, max = 64))]
	pub max_steps: i32,
	#[validate(range(min = 5, max = 900))]
	pub max_duration_secs: i32,
	#[validate(range(min = 1, max = 32))]
	pub max_concurrent: i32,
	#[validate(range(min = 1, max = 365))]
	pub payload_days: i32,
	#[validate(range(min = 1, max = 3650))]
	pub incident_evidence_days: i32,
}

pub(crate) fn simulated_mode() -> String {
	"simulated".into()
}

impl From<AgentTestSession> for TestSession {
	fn from(record: AgentTestSession) -> Self {
		Self {
			id: record.id,
			draft_id: record.draft_id(),
			tenant: record.tenant,
			revision: record.revision,
			status: record.status,
			scenario: record.scenario.into_inner(),
			conversation: record.conversation.map(|value| value.into_inner()),
			tool_calls: record.tool_calls.map(|value| value.into_inner()),
			usage: record.usage.into_inner(),
			error: record.error,
			created_at: record.created_at,
			updated_at: record.updated_at,
			expires_at: record.expires_at,
			expired_at: record.expired_at,
		}
	}
}

use uuid::Uuid;
