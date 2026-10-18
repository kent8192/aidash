//! Confined real-tool profiles a Tenant writes for its own test endpoints.
//! They carry no credential: a Deployment secret never reaches a Tenant's
//! endpoint, and Tenant Secrets for test endpoints come with #296.
use crate::registry::EntityRef;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RealToolRule {
	/// The profile writer's own claim that the test endpoint is read-only;
	/// never a publisher replay claim.
	#[serde(default)]
	pub read_only_verified: bool,
	pub tool: EntityRef,
	/// A test endpoint, distinct from the immutable production Tool endpoint.
	pub endpoint: String,
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
