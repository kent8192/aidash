//! Ordered File publication changes awaiting semantic indexing.
use chrono::{DateTime, Utc};
use reinhardt::{
	db::{associations::ForeignKeyField, orm::Json},
	model,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[model(app_label = "knowledge", table_name = "semantic_image_publications")]
#[derive(Serialize, Deserialize)]
pub struct SemanticImagePublication {
	#[field(primary_key = true)]
	pub sequence: i64,
	#[field(field_type = "text")]
	pub tenant: String,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "semantic_image_publications_workspace_fkey",
		db_index = false,
		on_delete = Cascade
	)]
	pub workspace: ForeignKeyField<crate::apps::workspaces::models::Workspace>,
	#[field]
	pub area_id: Uuid,
	#[field]
	pub generation: i64,
	#[field]
	pub changes: Json<Value>,
	#[field]
	pub authority: Json<Value>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}
