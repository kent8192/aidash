//! Persistent authorization_remote_grants records.

use chrono::{DateTime, Utc};
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "federation", table_name = "authorization_remote_grants")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationRemoteGrant {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "authorization_remote_grants_task_id_fkey",
		db_index = false,
		on_delete = Cascade
	)]
	pub task: ForeignKeyField<crate::apps::workspaces::models::Task>,
	#[field]
	pub task_revision: i64,
	#[field]
	pub workspace_id: uuid::Uuid,
	/// The Home Node that issued the grant.
	#[field(field_type = "text")]
	pub node: String,
	#[field(field_type = "text")]
	pub node_id: String,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub credential_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub root_subject: String,
	#[field]
	pub subject_chain: Vec<String>,
	#[field]
	pub inspection: Json<Value>,
	#[field]
	pub expires_at: DateTime<Utc>,
	#[field(default = false)]
	pub revoked: bool,

	#[field]
	pub semantic: Json<Value>,
}

impl AuthorizationRemoteGrant {}
