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
pub struct NativeBankProvenance {
	pub bank: crate::memory::Bank,
	pub provider: crate::registry::EntityRef,
	pub state: String,
	pub units: Vec<NativeUnitProvenance>,
}
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct NativeUnitProvenance {
	pub id: Uuid,
	pub revision: i64,
	pub kind: crate::memory::Kind,
	pub verification: crate::memory::Verification,
	pub evidence: Vec<crate::memory::Evidence>,
	pub content_digest: String,
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
	#[serde(skip_serializing_if = "Vec::is_empty")]
	pub memory: Vec<NativeBankProvenance>,
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
	/// Physical receiver quotation cleanup, independent of logical exclusion.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub body_cleanup: Option<String>,
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
		let memory = receipt
			.memory
			.into_iter()
			.flat_map(|memory| memory.banks)
			.map(|bank| {
				let (state, units) = match bank.recall {
					crate::memory::Recall::Ready { units } => ("ready", units),
					crate::memory::Recall::Empty => ("empty", vec![]),
					crate::memory::Recall::NoSpace => ("no_space", vec![]),
					crate::memory::Recall::Disabled => ("disabled", vec![]),
				};
				NativeBankProvenance {
					bank: bank.bank,
					provider: bank.provider,
					state: state.into(),
					units: units
						.into_iter()
						.map(|unit| NativeUnitProvenance {
							id: unit.id,
							revision: unit.revision,
							kind: unit.content.kind,
							verification: unit.content.verification,
							content_digest: crate::registry::rules::digest(&serde_json::json!(
								unit.content
							)),
							evidence: unit.content.evidence,
						})
						.collect(),
				}
			})
			.collect();
		Self {
			memory,
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
		body_cleanup: None,
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
			let native_count = receipt.memory.as_ref().map_or(0, |memory| {
				memory
					.banks
					.iter()
					.map(|bank| match &bank.recall {
						crate::memory::Recall::Ready { units } => units.len(),
						_ => 0,
					})
					.sum::<usize>()
			});
			let no_space = receipt.memory.as_ref().is_some_and(|memory| {
				memory
					.banks
					.iter()
					.any(|bank| matches!(bank.recall, crate::memory::Recall::NoSpace))
			});
			let count = receipt.result.matches.len() + native_count;
			result.result_count = Some(count);
			result.truncated = receipt.result.truncated || receipt.query_truncated;
			result.retrieved_at = Some(receipt.retrieved_at);
			result.state = if result.truncated {
				"truncated"
			} else if no_space && count == 0 {
				"no_space"
			} else if no_space {
				"truncated"
			} else if count == 0 {
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
