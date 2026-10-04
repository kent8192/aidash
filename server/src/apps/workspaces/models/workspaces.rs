//! Persistent workspaces records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "workspaces", table_name = "workspaces")]
#[derive(Serialize, Deserialize)]
pub struct Workspace {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub title: String,
	#[field(field_type = "text")]
	pub goal: String,
	#[field]
	pub state: Json<Value>,
	#[field(default = 0)]
	pub revision: i64,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}
