use serde::Deserialize;
// Serializable contracts for workbench.

use chrono::DateTime;
use chrono::Utc;
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

pub use aidash_domain::registry::workbench::report::{Report, ReportQuery};

pub use aidash_domain::registry::workbench::permissions::{
	PermissionContext, PermissionInput, PermissionRow,
};

use uuid::Uuid;
