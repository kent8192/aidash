//! Component-level factual permission context preserves worker policy and required Registry reads.
use crate::registry::EntityRef;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PermissionInput {
	pub tenant: String,
	pub subject: String,
	pub workspace_id: Option<Uuid>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PermissionRow {
	pub reference: EntityRef,
	pub kind: String,
	pub action: String,
	pub catalog_enabled: bool,
	pub policy_allowed: bool,
	pub registry_read_allowed: Option<bool>,
	pub effective_for_component: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PermissionContext {
	pub tenant: String,
	pub subject: String,
	pub workspace_id: Option<Uuid>,
	pub policy_revision: i64,
	pub observed_at: DateTime<Utc>,
	pub requested_capabilities: Vec<String>,
	pub rows: Vec<PermissionRow>,
	pub workspace_read: Option<bool>,
	pub note: String,
}
