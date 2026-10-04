//! Persistent authorization_execution records.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "authorization_execution")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationExecution {
	#[field(primary_key = true)]
	pub run_id: uuid::Uuid,
	#[field]
	pub task_id: uuid::Uuid,
	#[field]
	pub workspace_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub credential_id: uuid::Uuid,
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
