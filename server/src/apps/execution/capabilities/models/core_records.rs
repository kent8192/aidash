//! Persistent core_records records.
use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "execution", table_name = "core_records")]
#[derive(Serialize, Deserialize)]
pub struct CoreRecords {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub owner: String,
	#[field(null = true)]
	pub area_id: Option<uuid::Uuid>,
	#[field(field_type = "text")]
	pub kind: String,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub revision: i64,
	#[field]
	pub data: Json<Value>,
	#[field(null = true)]
	pub expires_at: Option<DateTime<Utc>>,
}
