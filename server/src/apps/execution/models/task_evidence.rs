//! Immutable task versions observed while preparing native memory inference.
use chrono::{DateTime, Utc};
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "execution", table_name = "run_task_snapshots")]
#[derive(Serialize, Deserialize)]
pub struct RunTaskSnapshot {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field]
	pub run_id: Uuid,
	#[field]
	pub task_revision: i64,
	#[field]
	pub step: i32,
	#[field]
	pub input_seq: i64,
	#[field]
	pub body: Json<aidash_domain::Task>,
	#[field]
	pub captured_at: DateTime<Utc>,
}
