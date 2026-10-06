//! Persistent task_dependencies records.

use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "workspaces", table_name = "task_dependencies")]
#[derive(Serialize, Deserialize)]
pub struct TaskDependency {
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(primary_key = true, db_column = "task_id", field_type = "uuid")]
	pub task_key: uuid::Uuid,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(db_column = "workspace_id")]
	pub workspace_key: uuid::Uuid,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(primary_key = true, db_column = "dependency_id", field_type = "uuid")]
	pub dependency_key: uuid::Uuid,
}
