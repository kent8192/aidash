//! Tenant-owned Scratch Space records.
use chrono::{DateTime, Utc};
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[model(app_label = "workspaces", table_name = "scratch_spaces")]
#[derive(Serialize, Deserialize)]
pub struct ScratchSpace {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub node: String,
	#[field(field_type = "text")]
	pub name: String,
	#[field(field_type = "text")]
	pub creator: String,
	#[field(field_type = "text")]
	pub state: String,
	pub created_at: DateTime<Utc>,
	pub last_used_at: DateTime<Utc>,
}

#[model(app_label = "workspaces", table_name = "scratch_space_settings")]
#[derive(Serialize, Deserialize)]
pub struct ScratchSpaceSettings {
	#[field(primary_key = true)]
	pub space_id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub settings: Json<Value>,
}

#[model(app_label = "workspaces", table_name = "scratch_space_members")]
#[derive(Serialize, Deserialize)]
pub struct ScratchSpaceMember {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	pub space_id: Uuid,
	#[field(field_type = "text")]
	pub subject: String,
}
