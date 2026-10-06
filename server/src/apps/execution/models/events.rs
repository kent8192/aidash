//! Persistent events records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::macros::Model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Model, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[model_config(app_label = "execution", table_name = "events")]
pub struct Event {
	#[field(primary_key = true)]
	pub sequence: i64,
	#[field]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub node_id: String,

	#[field(db_column = "workspace_id", null = true)]
	pub workspace_id: Option<uuid::Uuid>,
	#[field(field_type = "text")]
	pub kind: String,
	#[field]
	pub data: Json<Value>,
	#[field(null = true)]
	pub published_at: Option<DateTime<Utc>>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(auto_now_add = true)]
	pub next_attempt_at: DateTime<Utc>,
	#[field(field_type = "text", null = true)]
	pub publish_error: Option<String>,
}
