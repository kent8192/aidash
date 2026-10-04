//! Persistent authorization_remote_outputs records.
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "authorization_remote_outputs")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationRemoteOutputs {
	#[field(primary_key = true, field_type = "uuid")]
	pub grant_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "uuid")]
	pub workspace_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "text")]
	pub resource_kind: String,
	#[field(primary_key = true, field_type = "uuid")]
	pub resource_id: uuid::Uuid,
}
