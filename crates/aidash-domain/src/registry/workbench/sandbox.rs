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
	/// The revision the limits were read at. A write sends it back and fails
	/// with a conflict when another write came first.
	#[serde(default)]
	pub revision: i64,
}

/// A Tenant's Test Limits as it set them, with the Deployment's ceilings.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TenantTestLimits {
	pub limits: TestLimits,
	pub ceilings: TestLimitCeilings,
}

/// The Deployment's ceilings on every Tenant's Test Limits (#361). Tests run
/// on workers the Tenants share, so the Deployment bounds what one Tenant may
/// take, and each Tenant sets its own values within the ceilings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestLimitCeilings {
	pub max_input_bytes: i32,
	pub max_output_tokens: i32,
	pub max_total_tokens: i32,
	pub max_steps: i32,
	pub max_duration_secs: i32,
	pub max_concurrent: i32,
	pub payload_days: i32,
	pub incident_evidence_days: i32,
}

/// The smallest and largest value of each Test Limit, in field order.
const RANGES: [(&str, i32, i32); 8] = [
	("max_input_bytes", 1024, 1_000_000),
	("max_output_tokens", 64, 8192),
	("max_total_tokens", 256, 1_000_000),
	("max_steps", 1, 64),
	("max_duration_secs", 5, 900),
	("max_concurrent", 1, 32),
	("payload_days", 1, 365),
	("incident_evidence_days", 1, 3650),
];

impl TestLimitCeilings {
	/// The largest values a Tenant may set at all; the default ceilings.
	pub const MAXIMA: Self = Self {
		max_input_bytes: RANGES[0].2,
		max_output_tokens: RANGES[1].2,
		max_total_tokens: RANGES[2].2,
		max_steps: RANGES[3].2,
		max_duration_secs: RANGES[4].2,
		max_concurrent: RANGES[5].2,
		payload_days: RANGES[6].2,
		incident_evidence_days: RANGES[7].2,
	};

	fn values(&self) -> [i32; 8] {
		[
			self.max_input_bytes,
			self.max_output_tokens,
			self.max_total_tokens,
			self.max_steps,
			self.max_duration_secs,
			self.max_concurrent,
			self.payload_days,
			self.incident_evidence_days,
		]
	}

	/// Every ceiling lies within the range a Tenant may set at all.
	pub fn validate(&self) -> crate::Result<()> {
		for ((name, min, max), value) in RANGES.iter().zip(self.values()) {
			if !(*min..=*max).contains(&value) {
				return Err(crate::Error::Invalid(format!(
					"the {name} ceiling must be between {min} and {max}"
				)));
			}
		}
		Ok(())
	}

	/// Refuses Tenant values above a ceiling.
	pub fn permits(&self, limits: &TestLimits) -> crate::Result<()> {
		for ((name, ..), (value, ceiling)) in RANGES
			.iter()
			.zip(limit_values(limits).into_iter().zip(self.values()))
		{
			if value > ceiling {
				return Err(crate::Error::Invalid(format!(
					"{name} exceeds the Deployment's ceiling of {ceiling}"
				)));
			}
		}
		Ok(())
	}

	/// The limits that apply: stored values above a ceiling the Deployment
	/// lowered later are cut to it, and the output stays within the total.
	pub fn effective(&self, limits: TestLimits) -> TestLimits {
		let max_total_tokens = limits.max_total_tokens.min(self.max_total_tokens);
		TestLimits {
			max_input_bytes: limits.max_input_bytes.min(self.max_input_bytes),
			max_output_tokens: limits
				.max_output_tokens
				.min(self.max_output_tokens)
				.min(max_total_tokens),
			max_total_tokens,
			max_steps: limits.max_steps.min(self.max_steps),
			max_duration_secs: limits.max_duration_secs.min(self.max_duration_secs),
			max_concurrent: limits.max_concurrent.min(self.max_concurrent),
			payload_days: limits.payload_days.min(self.payload_days),
			incident_evidence_days: limits
				.incident_evidence_days
				.min(self.incident_evidence_days),
			..limits
		}
	}
}

impl Default for TestLimitCeilings {
	fn default() -> Self {
		Self::MAXIMA
	}
}

fn limit_values(limits: &TestLimits) -> [i32; 8] {
	[
		limits.max_input_bytes,
		limits.max_output_tokens,
		limits.max_total_tokens,
		limits.max_steps,
		limits.max_duration_secs,
		limits.max_concurrent,
		limits.payload_days,
		limits.incident_evidence_days,
	]
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
	if RANGES
		.iter()
		.zip(limit_values(value))
		.any(|((_, min, max), value)| !(*min..=*max).contains(&value))
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

/// A real-tool session pins the profile it was admitted with.
#[derive(Clone)]
pub struct ProfilePin {
	pub id: String,
	pub revision: i64,
	pub tenant: String,
	pub rules: Vec<super::profile::RealToolRule>,
}

/// A sandbox completion preserves the exact conversation, call evidence and observed usage.
#[derive(Debug, Clone)]
pub struct TestOutcome {
	pub status: String,
	pub conversation: Value,
	pub tool_calls: Value,
	pub usage: Value,
	pub error: Option<String>,
}
