//! Persistent authorization_threads records.

use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "authorization_threads")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationThread {
	#[field(primary_key = true)]
	pub thread_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub owner_subject: String,
}

impl AuthorizationThread {
	pub fn tenant_record_id(&self) -> String {
		self.tenant.clone()
	}
}
