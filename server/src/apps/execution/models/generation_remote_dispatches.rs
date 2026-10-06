//! Durable generation_remote_dispatches rows.
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "generation_remote_dispatches")]
#[derive(Serialize, Deserialize)]
pub struct GenerationRemoteDispatches {
	#[field(primary_key = true)]
	pub attempt_id: uuid::Uuid,
	#[field]
	pub usage: reinhardt::db::orm::Json<serde_json::Value>,
	#[field(field_type = "text")]
	pub digest: String,
	#[field(field_type = "text")]
	pub peer_node: String,
	#[field]
	pub boundary: reinhardt::db::orm::Json<serde_json::Value>,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub reservations: reinhardt::db::orm::Json<serde_json::Value>,
	#[field]
	pub finalization: Option<reinhardt::db::orm::Json<serde_json::Value>>,
	#[field]
	pub peer_finalized: bool,
	#[field]
	pub created_at: chrono::DateTime<chrono::Utc>,
}
