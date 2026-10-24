//! Persistent artifacts records.

use crate::apps::workspaces::services::states::ArtifactKind;
use chrono::{DateTime, Utc};
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "workspaces", table_name = "artifacts")]
#[derive(Serialize, Deserialize)]
pub struct Artifact {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "artifacts_workspace_id_fkey",
		db_index = false
	)]
	pub workspace: ForeignKeyField<crate::apps::workspaces::models::Workspace>,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "artifacts_task_id_fkey",
		db_index = false
	)]
	pub task: ForeignKeyField<crate::apps::workspaces::models::Task>,
	#[field(field_type = "text", max_length = 64)]
	pub kind: ArtifactKind,
	#[field(field_type = "text")]
	pub name: String,
	#[field]
	pub content: Json<Value>,
	#[field(field_type = "text")]
	pub created_by: String,
	#[field(field_type = "text")]
	pub idempotency_key: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	/// The Run that published this Artifact on this Node; see the contract.
	#[field(null = true)]
	pub run_id: Option<uuid::Uuid>,
}

impl Artifact {}
