//! Factual report exports combine independently authorized sources with an explicit sensitivity choice.
use super::{inspection::Inspection, permissions::PermissionContext};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Debug, Serialize, JsonSchema)]
pub struct Report {
	pub inspection: Inspection,
	pub permission_context: Option<PermissionContext>,
	pub incidents: Vec<crate::registry::workbench::incident::Incident>,
	pub exported_at: DateTime<Utc>,
	pub note: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ReportQuery {
	pub format: Option<String>,
	#[serde(default)]
	pub include_sensitive: bool,
	pub tenant: Option<String>,
	pub subject: Option<String>,
	pub workspace_id: Option<Uuid>,
}
