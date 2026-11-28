//! Task completion conditions and the evidence returned by the Goal judge.

use crate::{Error, Result, registry::EntityRef};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_GOAL_CHARACTERS: usize = 4_000;
pub const MAX_REASON_CHARACTERS: usize = 2_000;

pub fn validate_goal(goal: Option<&str>) -> Result<()> {
	if let Some(goal) = goal
		&& (goal.trim().is_empty() || goal.chars().count() > MAX_GOAL_CHARACTERS)
	{
		return Err(Error::Invalid(
			"Goal must contain 1–4,000 characters".into(),
		));
	}
	Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
	Met,
	NotYet,
	Impossible,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Judgment {
	pub verdict: Verdict,
	pub reason: String,
}

impl Judgment {
	pub fn parse(text: &str) -> Result<Self> {
		let judgment: Self = serde_json::from_str(text)?;
		if judgment.reason.trim().is_empty()
			|| judgment.reason.chars().count() > MAX_REASON_CHARACTERS
		{
			return Err(Error::Invalid(
				"Goal verdict reason must contain 1–2,000 characters".into(),
			));
		}
		Ok(judgment)
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct VerdictRecord {
	pub run_id: Uuid,
	pub response_epoch: i64,
	pub goal: String,
	pub judgment: Judgment,
	pub model: EntityRef,
	pub cost_nanocredits: Option<i64>,
	pub judged_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct Edit {
	pub goal: Option<String>,
	pub edited_by: String,
	pub edited_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct JudgingProgress {
	pub consecutive_not_yet: u8,
	pub last: Option<Box<VerdictRecord>>,
}

impl JudgingProgress {
	pub fn is_empty(&self) -> bool {
		self.consecutive_not_yet == 0 && self.last.is_none()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn goal_limits_count_unicode_characters() {
		assert!(validate_goal(Some(&"界".repeat(4_000))).is_ok());
		assert!(validate_goal(Some(&"界".repeat(4_001))).is_err());
		assert!(validate_goal(None).is_ok());
	}

	#[test]
	fn verdict_requires_a_supported_verdict_and_a_bounded_reason() {
		let reason = "界".repeat(2_000);
		let ready = serde_json::json!({"verdict": "not_yet", "reason": reason});
		assert_eq!(
			Judgment::parse(&ready.to_string()).unwrap().verdict,
			Verdict::NotYet
		);
		for invalid in [
			serde_json::json!({"verdict": "success", "reason": "Done"}),
			serde_json::json!({"verdict": "met", "reason": "界".repeat(2_001)}),
			serde_json::json!({"verdict": "met", "reason": "Done", "confidence": 1}),
		] {
			assert!(Judgment::parse(&invalid.to_string()).is_err());
		}
	}
}
