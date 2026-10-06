//! Durable semantic_remote_receipts rows.
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "knowledge", table_name = "semantic_remote_receipts")]
#[derive(Serialize, Deserialize)]
pub struct SemanticRemoteReceipts {
	#[field(primary_key = true)]
	pub operation_id: uuid::Uuid,
	#[field]
	pub run_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub receipt: reinhardt::db::orm::Json<serde_json::Value>,
	#[field]
	pub created_at: chrono::DateTime<chrono::Utc>,
}
