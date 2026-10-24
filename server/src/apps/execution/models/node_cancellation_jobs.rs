//! Durable, owner-bound stop work captured by the real Node deletion transaction.
use reinhardt::model;
use serde::{Deserialize, Serialize};
#[model(app_label = "execution", table_name = "node_cancellation_jobs")]
#[derive(Serialize, Deserialize)]
pub struct NodeCancellationJob {
	#[field(primary_key = true, field_type = "text")]
	pub id: String,
	#[field]
	pub deletion_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub node: String,
	#[field(field_type = "text")]
	pub kind: String,
	#[field]
	pub work_id: uuid::Uuid,
	#[field]
	pub binding: reinhardt::db::orm::Json<serde_json::Value>,
	#[field]
	pub retry_at: chrono::DateTime<chrono::Utc>,
}
