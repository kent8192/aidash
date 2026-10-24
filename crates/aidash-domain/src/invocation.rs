//! Provider-neutral durable invocation summaries, including the bounded diagnostic preview.
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
#[derive(Debug, Clone)]
pub struct InvocationSummary {
	pub idempotency_key: String,
	pub run_id: Uuid,
	pub tool: String,
	pub input: Value,
	pub status: String,
	pub result: Option<Value>,
	pub replay_safe: bool,
	pub created_at: DateTime<Utc>,
}
