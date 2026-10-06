//! Native field validation and ORM conversion for portable sandbox contracts.
use crate::apps::registry::workbench::models::AgentTestSession;
pub use aidash_domain::registry::workbench::sandbox::{
	Fixture, FixtureStatus, TestInput, TestSession,
};
use reinhardt::Validate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, Validate)]
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
crate::native_record!(TestLimits {
	tenant,
	max_input_bytes,
	max_output_tokens,
	max_total_tokens,
	max_steps,
	max_duration_secs,
	max_concurrent,
	payload_days,
	incident_evidence_days
});

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

impl From<TestLimits> for aidash_domain::registry::workbench::sandbox::TestLimits {
	fn from(value: TestLimits) -> Self {
		Self {
			tenant: value.tenant,
			max_input_bytes: value.max_input_bytes,
			max_output_tokens: value.max_output_tokens,
			max_total_tokens: value.max_total_tokens,
			max_steps: value.max_steps,
			max_duration_secs: value.max_duration_secs,
			max_concurrent: value.max_concurrent,
			payload_days: value.payload_days,
			incident_evidence_days: value.incident_evidence_days,
		}
	}
}

impl From<aidash_domain::registry::workbench::sandbox::TestLimits> for TestLimits {
	fn from(value: aidash_domain::registry::workbench::sandbox::TestLimits) -> Self {
		Self {
			tenant: value.tenant,
			max_input_bytes: value.max_input_bytes,
			max_output_tokens: value.max_output_tokens,
			max_total_tokens: value.max_total_tokens,
			max_steps: value.max_steps,
			max_duration_secs: value.max_duration_secs,
			max_concurrent: value.max_concurrent,
			payload_days: value.payload_days,
			incident_evidence_days: value.incident_evidence_days,
		}
	}
}
