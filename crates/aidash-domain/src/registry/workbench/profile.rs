//! Confined real-tool profiles contain credential references, never credential values.
use crate::registry::EntityRef;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RealToolRule {
	/// Operator attestation for the confined test endpoint; never a publisher replay claim.
	#[serde(default)]
	pub read_only_verified: bool,
	pub tool: EntityRef,
	/// A test endpoint, distinct from the immutable production Tool endpoint.
	pub endpoint: String,
	/// Environment variable name only. The secret value is never returned.
	pub credential_env: Option<String>,
	pub allowed_actions: Vec<String>,
	pub allowed_resources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TestProfile {
	pub tenant: String,
	pub id: String,
	pub revision: i64,
	pub enabled: bool,
	pub rules: Value,
	pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileInput {
	pub expected_revision: i64,
	pub enabled: bool,
	pub rules: Vec<RealToolRule>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProfileQuery {
	pub tenant: Option<String>,
	pub draft_id: Option<Uuid>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProfileSummary {
	pub id: String,
	pub revision: i64,
	pub enabled: bool,
}

use uuid::Uuid;
