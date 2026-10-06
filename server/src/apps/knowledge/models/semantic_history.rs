//! Persistent semantic_history records.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "knowledge", table_name = "semantic_history")]
#[derive(Serialize, Deserialize)]
pub struct SemanticHistory {
	#[field(primary_key = true)]
	pub sequence: i64,
	#[field]
	pub workspace_id: uuid::Uuid,
	#[field(null = true)]
	pub entry_id: Option<uuid::Uuid>,
	#[field]
	pub revision: i64,
	#[field(field_type = "text")]
	pub state: String,
	#[field(field_type = "text")]
	pub detail: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}
