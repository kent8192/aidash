use serde::{Deserialize, Serialize};
// Serializable contracts for workbench.

use crate::apps::registry::workbench::models::AgentTestProfile;
use crate::registry::EntityRef;
use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RealToolRule {
	pub tool: EntityRef,
	/// A test endpoint, distinct from the immutable production Tool endpoint.
	pub endpoint: String,
	/// Environment variable name only. The secret value is never returned.
	pub credential_env: Option<String>,
	pub allowed_actions: Vec<String>,
	pub allowed_resources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, JsonSchema)]
pub struct TestProfile {
	pub tenant: String,
	pub id: String,
	pub revision: i64,
	pub enabled: bool,
	pub rules: Value,
	pub updated_at: DateTime<Utc>,
}

impl From<AgentTestProfile> for TestProfile {
	fn from(row: AgentTestProfile) -> Self {
		Self {
			tenant: row.tenant,
			id: row.id,
			revision: row.revision,
			enabled: row.enabled,
			rules: row.rules.into_inner(),
			updated_at: row.updated_at,
		}
	}
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
