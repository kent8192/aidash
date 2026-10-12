//! Append-only Context Journal entries and pre-I/O Compaction Attempts.
use chrono::{DateTime, Utc};
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// One original Context Journal event; `runs.context` may later truncate it.
#[model(app_label = "execution", table_name = "run_context_events")]
#[derive(Serialize, Deserialize)]
pub struct RunContextEvent {
	#[field(primary_key = true)]
	pub run_id: Uuid,
	#[field(primary_key = true)]
	pub seq: i64,
	/// `appended` or `imported` from a projection saved before the journal.
	#[field(field_type = "text")]
	pub origin: String,
	#[field]
	pub event: Json<Value>,
	#[field(field_type = "text")]
	pub digest: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

/// A Compaction Attempt recorded before provider I/O; open while `outcome` is null.
#[model(app_label = "execution", table_name = "context_compaction_attempts")]
#[derive(Serialize, Deserialize)]
pub struct ContextCompactionAttempt {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field]
	pub run_id: Uuid,
	#[field(field_type = "text")]
	pub stage: String,
	#[field(field_type = "text")]
	pub policy_version: String,
	#[field(field_type = "text")]
	pub provider: String,
	#[field]
	pub source_from_seq: i64,
	#[field]
	pub source_through_seq: i64,
	#[field]
	pub base_revision: i64,
	#[field]
	pub observed_input_seq: i64,
	#[field]
	pub before_tokens: i64,
	#[field(field_type = "text", null = true)]
	pub outcome: Option<String>,
	#[field(field_type = "text", null = true)]
	pub reason: Option<String>,
	#[field(field_type = "text", null = true)]
	pub candidate_digest: Option<String>,
	#[field(null = true)]
	pub after_tokens: Option<i64>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(null = true)]
	pub settled_at: Option<DateTime<Utc>>,
}
