//! Persistent authorization_remote_admissions records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(
	app_label = "federation",
	table_name = "authorization_remote_admissions"
)]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationRemoteAdmission {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub source_node: String,
	#[field]
	pub grant_id: uuid::Uuid,
	#[field]
	pub task_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub credential_id: uuid::Uuid,
	#[field]
	pub subject_chain: Vec<String>,
	#[field]
	pub description: Json<Value>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}
