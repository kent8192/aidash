//! Durable semantic_remote_attempts rows.
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "knowledge", table_name = "semantic_remote_attempts")]
#[derive(Serialize, Deserialize)]
pub struct SemanticRemoteAttempts {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field]
	pub operation_id: uuid::Uuid,
	#[field]
	pub fence: i64,
	#[field]
	pub cycle: i32,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub reservations: reinhardt::db::orm::Json<serde_json::Value>,
	#[field(field_type = "text")]
	pub error: Option<String>,
	#[field]
	pub created_at: chrono::DateTime<chrono::Utc>,
	#[field]
	pub dispatched_at: Option<chrono::DateTime<chrono::Utc>>,
	#[field]
	pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}
