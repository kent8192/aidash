use serde::{Deserialize, Serialize};
// Serializable approvals contracts.
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Outbound {
	pub idempotency_key: Uuid,
	pub url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalChoice {
	#[default]
	AllowOnce,
	AllowRun,
	Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct ApprovalDecision {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	#[serde(default)]
	pub choice: ApprovalChoice,
	pub targets: Option<Vec<String>>,
	pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Revoke {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
}

use uuid::Uuid;
