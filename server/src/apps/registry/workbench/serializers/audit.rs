use serde::{Deserialize, Serialize};
// Serializable contracts for workbench.

use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde_json::Value;

#[derive(Deserialize, JsonSchema)]
pub(crate) struct AuditQuery {
	#[serde(default)]
	pub(crate) offset: usize,
	pub(crate) tenant: Option<String>,
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
