//! Evidence Revocation: the irreversible end of all of one Run's Research
//! evidence (ADR 0046). Stored copies of source content become tombstones;
//! identities, retrieval times and digests remain. The record of who revoked
//! it, when and why outlives the Run and holds no source content.
use super::{Failure, Operation};
use crate::{
	Error, Result,
	context::{ContextEvent, HistoryEntry},
	retention::Decider,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

/// The longest reason a revocation records, in characters.
pub const MAX_REASON_CHARS: usize = 1_000;

/// The policy action that permits a subject to revoke a Run's evidence,
/// evaluated on the `research` resource identified by the Run.
pub const REVOKE: &str = "research.revoke";

/// A revocation's required reason: trimmed, non-empty, bounded and free of
/// control characters other than line breaks.
pub fn reason(input: &str) -> Result<String> {
	let reason = input.trim();
	if reason.is_empty()
		|| reason.chars().count() > MAX_REASON_CHARS
		|| reason
			.chars()
			.any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
	{
		return Err(Error::Invalid(format!(
			"a revocation needs a reason of 1 to {MAX_REASON_CHARS} characters"
		)));
	}
	Ok(reason.to_owned())
}

/// What one revocation removed, counted and identified by digest only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Removed {
	pub sources: u32,
	pub snapshots: u32,
	pub fragments: u32,
	pub attempts: u32,
	pub invocations: u32,
	/// Text digests of the revoked Document snapshots, sorted and distinct.
	pub snapshot_digests: Vec<String>,
}

/// The audit record of one Evidence Revocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRevocation {
	pub run_id: Uuid,
	/// The Run's Tenant, when the Run has one.
	pub tenant: Option<String>,
	pub revoked_by: Decider,
	pub reason: String,
	pub revoked_at: DateTime<Utc>,
	pub removed: Removed,
}

/// What a revoked research result is replaced with wherever it was stored.
pub fn tombstone(operation: Operation) -> Value {
	super::failure(operation, Failure::EvidenceRevoked)
}

/// The research operation of a stored tool result, recognized by the shared
/// result envelope rather than by a tool name, so a result is recognized even
/// after its tool is no longer bound or loaded.
pub fn result_operation(result: &Value) -> Option<Operation> {
	if result["version"] != json!(1) || !result["limits"].is_object() {
		return None;
	}
	Operation::parse(result["operation"].as_str()?)
}

/// Whether `event` is a research result a model request would still carry.
/// A placeholder is not one, so redaction is idempotent.
pub fn carries_research(event: &ContextEvent) -> bool {
	matches!(event, ContextEvent::Tool { result, .. } if result_operation(result).is_some())
}

/// Replace every research result in `history` with the revoked placeholder,
/// keeping each call so its `tool_call_id` still pairs. Returns whether
/// anything was replaced.
pub fn redact(history: &mut [HistoryEntry]) -> bool {
	let mut redacted = false;
	for entry in history {
		if carries_research(&entry.event)
			&& let ContextEvent::Tool { call, result } = &mut entry.event
		{
			*result = ContextEvent::revoked_result(&call.id);
			redacted = true;
		}
	}
	redacted
}

#[cfg(test)]
mod tests;
