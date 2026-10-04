//! Durable invocation summaries exposed by the API.
use crate::apps::execution::models::{Invocation as InvocationRecord, states::InvocationStatus};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, JsonSchema)]
pub struct Invocation {
	pub idempotency_key: String,
	pub run_id: Uuid,
	pub tool: String,
	pub input: Value,
	pub status: String,
	pub result: Option<Value>,
	pub replay_safe: bool,
	pub created_at: DateTime<Utc>,
}

impl From<InvocationRecord> for Invocation {
	fn from(record: InvocationRecord) -> Self {
		Self {
			run_id: record.run_id(),
			idempotency_key: record.idempotency_key,
			tool: record.tool,
			input: record.input.into_inner(),
			status: match record.status {
				InvocationStatus::Started => "STARTED",
				InvocationStatus::Completed => "COMPLETED",
				InvocationStatus::Uncertain => "UNCERTAIN",
			}
			.into(),
			result: record.result.map(|value| value.into_inner()),
			replay_safe: record.replay_safe,
			created_at: record.created_at,
		}
	}
}

impl From<Invocation> for aidash_domain::invocation::InvocationSummary {
	fn from(value: Invocation) -> Self {
		Self {
			idempotency_key: value.idempotency_key,
			run_id: value.run_id,
			tool: value.tool,
			input: value.input,
			status: value.status,
			result: value.result,
			replay_safe: value.replay_safe,
			created_at: value.created_at,
		}
	}
}
impl From<aidash_domain::invocation::InvocationSummary> for Invocation {
	fn from(value: aidash_domain::invocation::InvocationSummary) -> Self {
		Self {
			idempotency_key: value.idempotency_key,
			run_id: value.run_id,
			tool: value.tool,
			input: value.input,
			status: value.status,
			result: value.result,
			replay_safe: value.replay_safe,
			created_at: value.created_at,
		}
	}
}
