//! Persistent authorization_workspaces records.

use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "authorization_workspaces")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationWorkspace {
	#[field(primary_key = true)]
	pub workspace_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub owner_subject: String,
}

impl AuthorizationWorkspace {
	pub fn tenant_record_id(&self) -> String {
		self.tenant.clone()
	}
}
