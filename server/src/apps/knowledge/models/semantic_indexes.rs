//! Persistent semantic_indexes records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "knowledge", table_name = "semantic_indexes")]
#[derive(Serialize, Deserialize)]
pub struct SemanticIndexe {
	#[field(primary_key = true)]
	pub workspace_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub revision: i64,
	#[field]
	pub spec: Json<Value>,
	#[field(field_type = "text")]
	pub collection: String,
	#[field(auto_now_add = true)]
	pub updated_at: DateTime<Utc>,
}

impl SemanticIndexe {}
