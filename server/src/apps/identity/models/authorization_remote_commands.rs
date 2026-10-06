//! Persistent authorization_remote_commands records.
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "identity", table_name = "authorization_remote_commands")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationRemoteCommands {
	#[field(primary_key = true, field_type = "uuid")]
	pub grant_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "text")]
	pub request_key: String,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub result: Json<Value>,
}
