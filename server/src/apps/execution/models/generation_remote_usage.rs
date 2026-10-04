//! Durable generation_remote_usage rows.
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "generation_remote_usage")]
#[derive(Serialize, Deserialize)]
pub struct GenerationRemoteUsage {
	#[field(primary_key = true, field_type = "uuid")]
	pub request_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "uuid")]
	pub attempt_id: uuid::Uuid,
	#[field]
	pub operation_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub dispatcher_node: String,
	#[field]
	pub grant_id: uuid::Uuid,
	#[field]
	pub admission_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub purpose: String,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub reserved_tokens: i64,
	#[field]
	pub reported_tokens: Option<i64>,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub created_at: chrono::DateTime<chrono::Utc>,
}
