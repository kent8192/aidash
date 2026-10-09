//! Inference Attempts and their display-only progress rows.
use chrono::{DateTime, Utc};
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// One provider call for one Run step. `usage_attempt` is the worker lease
/// token that fenced it; it is internal and never disclosed.
#[model(app_label = "execution", table_name = "inference_attempts")]
#[derive(Serialize, Deserialize)]
pub struct InferenceAttempt {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field]
	pub run_id: Uuid,
	#[field]
	pub usage_attempt: Uuid,
	#[field(field_type = "text", null = true)]
	pub outcome: Option<String>,
	#[field(field_type = "text", null = true)]
	pub reason: Option<String>,
	#[field]
	pub first_seq: i64,
	#[field]
	pub last_seq: i64,
	#[field]
	pub started_at: DateTime<Utc>,
	#[field(null = true)]
	pub finished_at: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub pruned_at: Option<DateTime<Utc>>,
}

/// One progress row at a per-Run `progress_seq`.
#[model(app_label = "execution", table_name = "inference_progress")]
#[derive(Serialize, Deserialize)]
pub struct InferenceProgressRecord {
	#[field(primary_key = true)]
	pub run_id: Uuid,
	#[field(primary_key = true)]
	pub seq: i64,
	#[field]
	pub attempt_id: Uuid,
	#[field(field_type = "text")]
	pub kind: String,
	#[field]
	pub item: Json<Value>,
	#[field]
	pub created_at: DateTime<Utc>,
}
