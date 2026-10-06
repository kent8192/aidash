//! Persistent agent_drafts records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "registry", table_name = "agent_drafts")]
#[derive(Serialize, Deserialize)]
pub struct AgentDraft {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub owner: String,
	#[field(field_type = "text")]
	pub managed_id: String,
	#[field(default = 1)]
	pub revision: i64,
	#[field]
	pub entry: Json<Value>,
	#[field]
	pub documents: Json<Value>,
	#[field(field_type = "text")]
	pub release_notes: String,
	#[field(field_type = "text", null = true)]
	pub source_id: Option<String>,
	#[field(field_type = "text", null = true)]
	pub source_version: Option<String>,
	#[field(default = false)]
	pub archived: bool,
	#[field(auto_now_add = true)]
	pub updated_at: DateTime<Utc>,
}
