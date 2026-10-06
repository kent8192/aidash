//! Factual connected-node usage and sandbox evidence; no Trust verdict is inferred.
use crate::registry::Entry;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;
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

/// A storage-independent sandbox observation used to construct factual evidence.
#[derive(Debug, Clone)]
pub struct TestObservation {
	pub id: Uuid,
	pub status: String,
	pub scenario: Value,
	pub usage: Value,
	pub created_at: DateTime<Utc>,
	pub expires_at: DateTime<Utc>,
	pub expired_at: Option<DateTime<Utc>>,
}
