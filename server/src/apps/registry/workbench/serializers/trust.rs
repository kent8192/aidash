use serde::{Deserialize, Serialize};
// Serializable contracts for workbench.

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

pub use aidash_domain::registry::workbench::inspection::{Inspection, TestEvidence, WorkspaceUse};

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
