//! Persistent authorization_run_outputs records.

use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "authorization_run_outputs")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationRunOutput {
	#[field(primary_key = true, field_type = "uuid")]
	pub run_id: uuid::Uuid,
	#[field]
	pub workspace_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "text")]
	pub resource_kind: String,
	#[field(primary_key = true, field_type = "uuid")]
	pub resource_id: uuid::Uuid,
}

impl AuthorizationRunOutput {}
