//! Persistent tasks records.

use crate::apps::workspaces::services::states::TaskStatus;
use chrono::{DateTime, Utc};
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[model(app_label = "workspaces", table_name = "tasks")]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,

	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "tasks_workspace_id_fkey",
		db_index = false
	)]
	pub workspace: ForeignKeyField<crate::apps::workspaces::models::Workspace>,
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

	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "tasks_parent_id_fkey",
		db_index = false,
		null = true
	)]
	pub parent: ForeignKeyField<crate::apps::workspaces::models::Task>,
	#[field(default = 0)]
	pub revision: i64,
	#[field(field_type = "text", null = true)]
	pub creation_key: Option<String>,
	#[field(field_type = "text", null = true)]
	pub completion_key: Option<String>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}
