//! Persistent retention holds that keep a File version after it is superseded.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "core_retention_holds")]
#[derive(Serialize, Deserialize)]
pub struct CoreRetentionHolds {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	pub area_id: uuid::Uuid,
	pub revision: i64,
	#[field(field_type = "text")]
	pub holder_kind: String,
	#[field(field_type = "text")]
	pub holder_id: String,
	#[field(field_type = "text", null = true)]
	pub peer: Option<String>,
	#[field(null = true)]
	pub expires_at: Option<DateTime<Utc>>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

#[model(app_label = "execution", table_name = "core_retention_files")]
#[derive(Serialize, Deserialize)]
pub struct CoreRetentionFiles {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	pub hold_id: uuid::Uuid,
	pub file_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub path: String,
	pub size: i64,
	#[field(field_type = "text")]
	pub media_type: String,
}
