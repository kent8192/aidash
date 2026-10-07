//! Persistent authorization_remote_execution records.
use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "identity", table_name = "authorization_remote_execution")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationRemoteExecution {
	#[field(primary_key = true)]
	pub grant_id: uuid::Uuid,
	#[field]
	pub admission_id: uuid::Uuid,
	#[field]
	pub task_id: uuid::Uuid,
	#[field]
	pub task_revision: i64,
	#[field]
	pub initial_task: Json<Value>,
	#[field]
	pub human_requests: Json<Value>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}
