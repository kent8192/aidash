//! Remote summaries disclose provenance without retaining query or source text.
use super::{Binding, Receipt, journal::Record};
use crate::semantic::Failure;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "RemoteSemanticSourceProvenance")]
pub struct SourceProvenance {
	pub entry_id: Uuid,
	pub revision: i64,
	pub content_digest: String,
	pub agent: Option<String>,
}
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "RemoteSemanticProvenance")]
pub struct Provenance {
	pub home_node: String,
	pub operation_id: Uuid,
	pub executor: String,
	pub binding: Binding,
	pub model: String,
	pub model_version: String,
	pub retrieved_at: DateTime<Utc>,
	pub truncated: bool,
	pub sources: Vec<SourceProvenance>,
	/// Current, authorized counters at the node serving this view. Foreign
	/// balances are never presented as authoritative cached counters.
	pub allowance_node: String,
	pub allowances: Vec<Allowance>,
}
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "RemoteSemanticAllowance")]
pub struct Allowance {
	pub request_id: Uuid,
	pub token_limit: i64,
	pub used_tokens: i64,
	pub embedding_call_limit: i64,
	pub embedding_calls: i64,
	pub compaction_call_limit: i64,
	pub compaction_calls: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "RemoteSemanticStatus")]
pub struct Status {
	pub state: String,
	pub reason: Option<Failure>,
	pub operation_id: Option<Uuid>,
	pub retry_count: i32,
	pub retry_at: Option<DateTime<Utc>>,
	pub result_count: Option<usize>,
	pub truncated: bool,
	pub retrieved_at: Option<DateTime<Utc>>,
}
impl From<Receipt> for Provenance {
	fn from(receipt: Receipt) -> Self {
		Self {
			allowance_node: String::new(),
			allowances: vec![],
			home_node: receipt.home_node,
			operation_id: receipt.operation_id,
			executor: receipt.executor,
			binding: receipt.binding,
			model: receipt.result.model,
			model_version: receipt.result.model_version,
			retrieved_at: receipt.retrieved_at,
			truncated: receipt.query_truncated || receipt.result.truncated,
			sources: receipt
				.sources
				.into_iter()
				.zip(receipt.result.matches)
				.map(|(source, found)| SourceProvenance {
					entry_id: source.entry_id,
					revision: source.revision,
					content_digest: source.content_digest,
					agent: found.agent,
				})
				.collect(),
		}
	}
}
pub fn project(
	binding: &Binding,
	reason: Option<Failure>,
	record: Option<Record>,
) -> serde_json::Result<Status> {
	let mut result = Status {
		state: if binding.disabled() {
			"disabled"
		} else {
			"pending"
		}
		.into(),
		reason,
		operation_id: None,
		retry_count: 0,
		retry_at: None,
		result_count: None,
		truncated: false,
		retrieved_at: None,
	};
	if binding.disabled() {
		return Ok(result);
	}
	if let Some(record) = record {
		result.state = record.state.to_lowercase();
		result.operation_id = Some(record.id);
		result.retry_count = record.failures.min(5);
		result.retry_at = record.next_attempt;
		result.reason = result.reason.or_else(|| {
			record
				.error
				.and_then(|s| serde_json::from_value(serde_json::json!(s)).ok())
		});
		if result.reason.is_none()
			&& let Some(value) = record.receipt
		{
			let receipt: Receipt = serde_json::from_value(value)?;
			result.result_count = Some(receipt.result.matches.len());
			result.truncated = receipt.result.truncated || receipt.query_truncated;
			result.retrieved_at = Some(receipt.retrieved_at);
			result.state = if result.truncated {
				"truncated"
			} else if receipt.result.matches.is_empty() {
				"empty"
			} else {
				"ready"
			}
			.into();
		}
	}
	if let Some(reason) = result.reason {
		result.state = if reason == Failure::Invalidated {
			"invalidated"
		} else if result.retry_at.is_some() {
			"waiting"
		} else {
			"paused"
		}
		.into();
	}
	Ok(result)
}
#[cfg(test)]
mod tests;
