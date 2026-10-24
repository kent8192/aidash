//! Persistent core_objects records.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "core_objects")]
#[derive(Serialize, Deserialize)]
pub struct CoreObjects {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(null = true)]
	pub area_id: Option<uuid::Uuid>,
	#[field(field_type = "text")]
	pub kind: String,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub size: i64,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}
