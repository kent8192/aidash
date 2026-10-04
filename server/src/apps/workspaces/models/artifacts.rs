//! Persistent artifacts records.

use crate::apps::workspaces::services::states::ArtifactKind;
use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "workspaces", table_name = "artifacts")]
#[derive(Serialize, Deserialize)]
pub struct Artifact {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field]
	pub workspace_id: uuid::Uuid,
	#[field]
	pub task_id: uuid::Uuid,
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
}

impl Artifact {}
