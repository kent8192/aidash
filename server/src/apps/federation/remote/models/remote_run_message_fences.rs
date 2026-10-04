//! Persistent remote_run_message_fences records.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "federation", table_name = "remote_run_message_fences")]
#[derive(Serialize, Deserialize)]
pub struct RemoteRunMessageFence {
	#[field(primary_key = true, field_type = "uuid")]
	pub task_id: uuid::Uuid,
	#[field]
	pub run_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "text")]
	pub idempotency_key: String,
	#[field(field_type = "text")]
	pub content: String,
	#[field(null = true)]
	pub input_seq: Option<i64>,
	#[field(null = true)]
	pub expires_at: Option<DateTime<Utc>>,
	#[field(default = false)]
	pub consumed: bool,
}

impl RemoteRunMessageFence {}
