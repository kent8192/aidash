//! Persistent authorization_task_origins records.

use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "authorization_task_origins")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationTaskOrigin {
	#[field(primary_key = true)]
	pub task_id: uuid::Uuid,
	/// Historical source UUID; insertion checks same-Tenant existence.
	#[field]
	pub source_run_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub root_subject: String,
	#[field]
	pub subject_chain: Vec<String>,
	/// Recorded when the origin is inserted: the creating chain passed
	/// through another Node or could not be proven Local (#1012).
	#[field(default = false)]
	pub federated: bool,
}

impl AuthorizationTaskOrigin {
	pub fn tenant_record_id(&self) -> String {
		self.tenant.clone()
	}
}
