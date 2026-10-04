//! Durable semantic_remote_operations rows.
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "knowledge", table_name = "semantic_remote_operations")]
#[derive(Serialize, Deserialize)]
pub struct SemanticRemoteOperations {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub home_node: String,
	#[field]
	pub grant_id: uuid::Uuid,
	#[field]
	pub admission_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub binding: reinhardt::db::orm::Json<serde_json::Value>,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub cycle: i32,
	#[field]
	pub failures: i32,
	#[field]
	pub attempt_id: Option<uuid::Uuid>,
	#[field]
	pub fence: i64,
	#[field]
	pub lease_until: Option<chrono::DateTime<chrono::Utc>>,
	#[field]
	pub next_attempt: Option<chrono::DateTime<chrono::Utc>>,
	#[field(field_type = "text")]
	pub error: Option<String>,
	#[field]
	pub receipt: Option<reinhardt::db::orm::Json<serde_json::Value>>,
	#[field]
	pub created_at: chrono::DateTime<chrono::Utc>,
}
