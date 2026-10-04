//! Persistent semantic_collections records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "knowledge", table_name = "semantic_collections")]
#[derive(Serialize, Deserialize)]
pub struct SemanticCollection {
	#[field(primary_key = true, field_type = "text")]
	pub collection: String,
	#[field]
	pub workspace_id: uuid::Uuid,
	#[field]
	pub vector: Json<Value>,
	#[field(default = false)]
	pub retired: bool,
	#[field(field_type = "text", null = true)]
	pub last_error: Option<String>,
	#[field(null = true)]
	pub cleaned_at: Option<DateTime<Utc>>,
	#[field(auto_now_add = true)]
	pub next_attempt: DateTime<Utc>,
}

impl SemanticCollection {}
