//! Persistent threads records.

use chrono::{DateTime, Utc};
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "workspaces", table_name = "threads")]
#[derive(Serialize, Deserialize)]
pub struct Thread {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	/// The Node of the Tenant that the Thread belongs to.
	#[field(field_type = "text")]
	pub node: String,
	#[rel(foreign_key, shared_fields = ["tenant"], db_constraint = "threads_scratch_space_fkey", db_index = false)]
	pub scratch_space: ForeignKeyField<crate::apps::workspaces::models::ScratchSpace>,
	#[field(field_type = "text")]
	pub created_by: String,
	#[field(field_type = "text")]
	pub title: String,
	#[field(default = 0)]
	pub revision: i64,
	/// When its owner or a Node administrator deleted it; every read and
	/// write of a deleted Thread refuses (#1008).
	#[field(null = true)]
	pub deleted_at: Option<DateTime<Utc>>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}
