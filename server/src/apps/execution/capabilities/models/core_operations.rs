//! Persistent core_operations records.
use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "execution", table_name = "core_operations")]
#[derive(Serialize, Deserialize)]
pub struct CoreOperations {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field]
	pub area_id: uuid::Uuid,
	#[field]
	pub run_id: uuid::Uuid,
	#[field]
	pub credential_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub principal: String,
	#[field(field_type = "text")]
	pub request_key: String,
	#[field(field_type = "text")]
	pub digest: String,
	#[field(field_type = "text")]
	pub kind: String,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub epoch: i64,
	#[field]
	pub generation: i64,
	#[field]
	pub revision: i64,
	#[field]
	pub policy_revision: i64,
	#[field]
	pub subjects: Json<Value>,
	#[field]
	pub input: Json<Value>,
	#[field]
	pub result: Json<Value>,
	#[field(field_type = "text", null = true)]
	pub runner_instance: Option<String>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(auto_now_add = true)]
	pub updated_at: DateTime<Utc>,
}
