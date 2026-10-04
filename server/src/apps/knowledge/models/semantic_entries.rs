//! Persistent semantic_entries records.

use crate::apps::knowledge::services::states::SemanticEntryState;
use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "knowledge", table_name = "semantic_entries")]
#[derive(Serialize, Deserialize)]
pub struct SemanticEntry {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field]
	pub workspace_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub key: String,
	#[field]
	pub source: Json<Value>,
	#[field(field_type = "text", null = true)]
	pub agent: Option<String>,
	#[field]
	pub metadata: Json<Value>,
	#[field]
	pub revision: i64,
	#[field]
	pub point_id: uuid::Uuid,
	#[field]
	pub index_revision: i64,
	#[field(default = false)]
	pub deleted: bool,
	#[field(field_type = "text", max_length = 64)]
	pub state: SemanticEntryState,
	#[field(default = 0)]
	pub attempts: i32,
	#[field(field_type = "text", null = true)]
	pub last_error: Option<String>,
	#[field(field_type = "text")]
	pub created_by: String,
	#[field]
	pub authority: Json<Value>,
	#[field(auto_now_add = true)]
	pub updated_at: DateTime<Utc>,
	#[field(auto_now_add = true)]
	pub next_attempt: DateTime<Utc>,
}

impl SemanticEntry {}
