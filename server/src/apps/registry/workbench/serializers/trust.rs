use serde::{Deserialize, Serialize};
// Serializable contracts for workbench.

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

pub use aidash_domain::registry::workbench::permissions::{
	PermissionContext, PermissionInput, PermissionRow,
};

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
