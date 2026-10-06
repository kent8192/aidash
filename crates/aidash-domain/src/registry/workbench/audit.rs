//! Source-attributed Workbench history has the same bounded wire contract.
use serde::{Deserialize, Serialize};
// Serializable contracts for workbench.

use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde_json::Value;

#[derive(Deserialize, JsonSchema)]
pub struct AuditQuery {
	#[serde(default)]
	pub offset: usize,
	pub tenant: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AuditItem {
	pub source: String,
	pub kind: String,
	pub at: DateTime<Utc>,
	pub actor: Option<String>,
	pub details: Value,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AuditPage {
	pub observed_at: DateTime<Utc>,
	pub items: Vec<AuditItem>,
	pub next_offset: Option<usize>,
	pub source_boundary: String,
}

use uuid::Uuid;
#[derive(Debug, Clone)]
pub struct Registration {
	pub draft_id: Uuid,
	pub revision: i64,
	pub actor: String,
	pub registered_at: DateTime<Utc>,
}
#[derive(Debug, Clone)]
pub struct TestRecord {
	pub id: Uuid,
	pub status: String,
	pub usage: Value,
	pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone)]
pub struct CatalogChange {
	pub revision: i64,
	pub enabled: bool,
	pub actor: String,
	pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone)]
pub struct IncidentChange {
	pub actor: String,
	pub change: Value,
	pub created_at: DateTime<Utc>,
}
