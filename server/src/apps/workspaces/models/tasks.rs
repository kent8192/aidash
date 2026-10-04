//! Persistent tasks records.

use crate::apps::workspaces::services::states::TaskStatus;
use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::macros::Model;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Model, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[model_config(app_label = "workspaces", table_name = "tasks")]
pub struct Task {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,

	#[field(db_column = "workspace_id")]
	pub workspace_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub title: String,
	#[field(field_type = "text")]
	pub description: String,
	#[field(field_type = "text", max_length = 64)]
	pub status: TaskStatus,
	#[field]
	pub requirements: Json<Value>,
	#[field(field_type = "text", null = true)]
	pub owner: Option<String>,
	#[field(field_type = "text")]
	pub created_by: String,
	#[field]
	pub dependencies: Vec<Uuid>,

	#[field(db_column = "parent_id", null = true)]
	pub parent_id: Option<uuid::Uuid>,
	#[field(default = 0)]
	pub revision: i64,
	#[field(field_type = "text", null = true)]
	pub creation_key: Option<String>,
	#[field(field_type = "text", null = true)]
	pub completion_key: Option<String>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}
