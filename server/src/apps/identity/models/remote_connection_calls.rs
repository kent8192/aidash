//! Home-owned invocation metadata for Runs whose executable state lives on another Node.
use chrono::{DateTime, Utc};
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
#[model(app_label = "identity", table_name = "remote_connection_calls")]
#[derive(Serialize, Deserialize)]
pub struct RemoteConnectionCall {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub grant_id: Uuid,
	#[field]
	pub admission_id: Uuid,
	#[field(field_type = "text")]
	pub idempotency_key: String,
	#[field]
	pub tool_key: Json<Value>,
	#[field(field_type = "text")]
	pub resource: String,
	#[field(field_type = "text")]
	pub arguments_hash: String,
	#[field(null = true)]
	pub connection_id: Option<Uuid>,
	#[field(null = true)]
	pub secret_id: Option<Uuid>,
	#[field(field_type = "text", null = true)]
	pub secret_version: Option<String>,
	#[field(field_type = "text", null = true)]
	pub secret_host: Option<String>,
	#[field(field_type = "text")]
	pub status: String,
	#[field(null = true)]
	pub result: Option<Json<Value>>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(auto_now = true)]
	pub updated_at: DateTime<Utc>,
}
