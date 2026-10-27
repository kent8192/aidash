//! Persistent citations from a message to exact File versions.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "workspaces", table_name = "message_file_versions")]
#[derive(Serialize, Deserialize)]
pub struct MessageFileVersion {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	pub message_id: uuid::Uuid,
	pub file_id: uuid::Uuid,
	#[field(null = true)]
	pub remote_run_id: Option<uuid::Uuid>,
	#[field(null = true)]
	pub remote_expires_at: Option<DateTime<Utc>>,
	#[field(field_type = "text")]
	pub tenant: String,
	pub area_id: uuid::Uuid,
	pub revision: i64,
	#[field(field_type = "text")]
	pub path: String,
	#[field(field_type = "text")]
	pub digest: String,
	pub size: i64,
	#[field(field_type = "text")]
	pub media_type: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}
