//! Durable generation_remote_intents rows.
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "generation_remote_intents")]
#[derive(Serialize, Deserialize)]
pub struct GenerationRemoteIntents {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field]
	pub task_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub credential_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub root_subject: String,
	#[field]
	pub subject_chain: Vec<String>,
	#[field]
	pub binding: reinhardt::db::orm::Json<serde_json::Value>,
	#[field]
	pub cancelled: bool,
	#[field]
	pub cancel_delivered: bool,
	#[field]
	pub cancel_retry_at: Option<chrono::DateTime<chrono::Utc>>,
	#[field]
	pub created_at: chrono::DateTime<chrono::Utc>,
}
