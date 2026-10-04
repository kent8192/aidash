//! Sandbox sessions retain portable fixtures, pinned continuation facts and execution limits.
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestLimits {
	pub tenant: String,
	pub max_input_bytes: i32,
	pub max_output_tokens: i32,
	pub max_total_tokens: i32,
	pub max_steps: i32,
	pub max_duration_secs: i32,
	pub max_concurrent: i32,
	pub payload_days: i32,
	pub incident_evidence_days: i32,
}

fn simulated_mode() -> String {
	"simulated".into()
}

pub fn validate_request(input: &TestInput) -> crate::Result<()> {
	if input.message.trim().is_empty() || input.fixtures.len() > 64 {
		return Err(crate::Error::Invalid(
			"test requires a message and at most 64 fixtures".into(),
		));
	}
	if !matches!(input.mode.as_str(), "simulated" | "real") {
		return Err(crate::Error::Invalid(
			"test mode must be simulated or real".into(),
		));
	}
	if input.mode == "simulated" && input.profile_id.is_some() {
		return Err(crate::Error::Invalid(
			"simulated tests cannot select a real-tool profile".into(),
		));
	}
	Ok(())
}
pub fn validate_limits(value: &TestLimits) -> crate::Result<()> {
	if !(1024..=1_000_000).contains(&value.max_input_bytes)
		|| !(64..=8192).contains(&value.max_output_tokens)
		|| !(256..=1_000_000).contains(&value.max_total_tokens)
		|| !(1..=64).contains(&value.max_steps)
		|| !(5..=900).contains(&value.max_duration_secs)
		|| !(1..=32).contains(&value.max_concurrent)
		|| !(1..=365).contains(&value.payload_days)
		|| !(1..=3650).contains(&value.incident_evidence_days)
		|| value.max_total_tokens < value.max_output_tokens
	{
		return Err(crate::Error::Invalid(
			"test limits are outside the supported ranges".into(),
		));
	}
	Ok(())
}
pub fn has_unknown_call(calls: &Option<Value>) -> bool {
	calls
		.as_ref()
		.and_then(Value::as_array)
		.is_some_and(|items| {
			items
				.iter()
				.any(|item| item["outcome"] == "outcome_unknown")
		})
}
pub fn continued_conversation(
	previous: TestSession,
	draft: &super::Draft,
	input: &TestInput,
	profile_revision: Option<i64>,
) -> crate::Result<Vec<Value>> {
	if previous.draft_id != draft.id
		|| previous.revision != draft.revision
		|| previous.status != "completed"
		|| previous.expired_at.is_some()
		|| previous.scenario["mode"] != input.mode
		|| previous.scenario["profile_id"] != serde_json::to_value(&input.profile_id)?
		|| previous.scenario["profile_revision"] != serde_json::to_value(profile_revision)?
	{
		return Err(crate::Error::Conflict(
			"previous test context is stale or unavailable; reset the conversation".into(),
		));
	}
	previous
		.conversation
		.and_then(|value| value.as_array().cloned())
		.ok_or_else(|| crate::Error::Conflict("previous test conversation is unavailable".into()))
}
#[cfg(test)]
mod tests;

/// A real-tool session pins configuration and credential fingerprints, not secret values.
#[derive(Clone)]
pub struct ProfilePin {
	pub id: String,
	pub revision: i64,
	pub tenant: String,
	pub rules: Vec<super::profile::RealToolRule>,
	pub credential_fingerprints: BTreeMap<String, Vec<u8>>,
}
