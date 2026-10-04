use serde::{Deserialize, Serialize};
// Serializable contracts for workbench.

use crate::registry::EntityRef;
use crate::registry::Entry;
use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde_json::Value;

#[derive(Debug, Deserialize, sqlx::FromRow)]
pub(crate) struct EvidenceRow {
	pub id: Uuid,
	pub status: String,
	pub scenario: Value,
	pub usage: Value,
	pub created_at: DateTime<Utc>,
	pub expires_at: DateTime<Utc>,
	pub expired_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WorkspaceUse {
	pub workspace_id: Uuid,
	pub title: String,
	pub current: bool,
	pub latest_run_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "WorkbenchTrustInspection")]
pub struct Inspection {
	pub entry: Entry,
	pub source_node: String,
	pub observed_at: DateTime<Utc>,
	pub workspaces: Vec<WorkspaceUse>,
	pub usage_truncated: bool,
	pub test_evidence: Vec<TestEvidence>,
	pub test_evidence_truncated: bool,
	pub external_assessment_available: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TestEvidence {
	pub session_id: Uuid,
	pub draft_revision: i64,
	pub mode: String,
	pub profile_id: Option<String>,
	pub profile_revision: Option<i64>,
	pub status: String,
	pub usage: Value,
	pub created_at: DateTime<Utc>,
	pub expires_at: DateTime<Utc>,
	pub expired_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Report {
	pub inspection: Inspection,
	pub permission_context: Option<PermissionContext>,
	pub incidents: Vec<crate::workbench::incident::Incident>,
	pub exported_at: DateTime<Utc>,
	pub note: String,
}

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

#[derive(Deserialize, JsonSchema)]
pub(crate) struct ReportQuery {
	pub(crate) format: Option<String>,
	#[serde(default)]
	pub(crate) include_sensitive: bool,
	pub(crate) tenant: Option<String>,
	pub(crate) subject: Option<String>,
	pub(crate) workspace_id: Option<Uuid>,
}

use uuid::Uuid;
