//! Persistent authorization_execution records.

use chrono::{DateTime, Utc};
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "authorization_execution")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationExecution {
	#[field(primary_key = true)]
	pub run_id: uuid::Uuid,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "authorization_execution_task_id_fkey",
		db_index = false
	)]
	pub task: ForeignKeyField<crate::apps::workspaces::models::Task>,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "authorization_execution_workspace_id_fkey",
		db_index = false
	)]
	pub workspace: ForeignKeyField<crate::apps::identity::models::AuthorizationWorkspace>,
	#[field(field_type = "text")]
	pub tenant: String,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "authorization_execution_credential_id_fkey",
		db_index = false
	)]
	pub credential: ForeignKeyField<crate::apps::identity::models::AuthorizationCredential>,
	#[field(field_type = "text")]
	pub root_subject: String,
	#[field]
	pub subject_chain: Vec<String>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

impl AuthorizationExecution {
	pub fn tenant_record_id(&self) -> String {
		self.tenant.clone()
	}
}
