//! Persistent authorization_run_remote_reads records.

use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "identity", table_name = "authorization_run_remote_reads")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationRunRemoteRead {
	#[field(primary_key = true, field_type = "uuid")]
	pub run_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "text")]
	pub node_id: String,
	#[field(primary_key = true, field_type = "text")]
	pub entry_id: String,
	#[field(primary_key = true, field_type = "text")]
	pub entry_version: String,
	#[field(primary_key = true, field_type = "text")]
	pub digest: String,
	#[field]
	pub metadata: Json<Value>,
}

impl AuthorizationRunRemoteRead {}
