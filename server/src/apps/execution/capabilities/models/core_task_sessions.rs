//! Persistent core_task_sessions records.
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "core_task_sessions")]
#[derive(Serialize, Deserialize)]
pub struct CoreTaskSessions {
	#[field(primary_key = true)]
	pub task_id: uuid::Uuid,
	#[field]
	pub thread_id: uuid::Uuid,
}
