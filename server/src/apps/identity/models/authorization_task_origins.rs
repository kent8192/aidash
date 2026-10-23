//! Persistent authorization_task_origins records.

use reinhardt::db::associations::ForeignKeyField;
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "authorization_task_origins")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationTaskOrigin {
	#[field(primary_key = true)]
	pub task_id: uuid::Uuid,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "authorization_task_origins_source_run_id_fkey",
		db_index = false
	)]
	pub source_run: ForeignKeyField<crate::apps::execution::models::Run>,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub root_subject: String,
	#[field]
	pub subject_chain: Vec<String>,
}

impl AuthorizationTaskOrigin {
	pub fn tenant_record_id(&self) -> String {
		self.tenant.clone()
	}
}
