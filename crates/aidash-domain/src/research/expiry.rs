//! Provider results the Research account terms no longer keep (ADR 0058).
//! Stored copies become tombstones; in a Run's projection each research
//! result built from an expired Provider result becomes a placeholder, while
//! its call's arguments, which Aidash itself sent, stay. The Context Journal
//! is never edited.
use super::{Failure, Operation, revocation::result_operation};
use crate::context::{ContextEvent, HistoryEntry};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use uuid::Uuid;

/// What a stored expired Provider result is replaced with.
pub fn tombstone(operation: Operation) -> Value {
	super::failure(operation, Failure::ResultExpired)
}

/// The expired Provider results of one Run, by the identities its research
/// results name: the Source records a Web search created, and the Document
/// snapshots a provider with account terms read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpiredResults {
	pub sources: BTreeSet<Uuid>,
	pub documents: BTreeSet<Uuid>,
	/// The latest of these purges.
	pub through: Option<DateTime<Utc>>,
}

impl ExpiredResults {
	pub fn is_empty(&self) -> bool {
		self.sources.is_empty() && self.documents.is_empty()
	}

	/// Whether a research result carries an expired Provider result. A search
	/// result is matched by the Sources it created and a read, find or view
	/// result by its Document snapshot only, so reading a searched Source with
	/// Aidash's own page reader never expires with the search.
	pub fn covers(&self, result: &Value) -> bool {
		let uuid = |value: &Value| value.as_str().and_then(|id| id.parse::<Uuid>().ok());
		match result_operation(result) {
			Some(Operation::Search) => {
				result["data"]["sources"].as_array().is_some_and(|sources| {
					sources
						.iter()
						.filter_map(|source| uuid(&source["source_id"]))
						.any(|id| self.sources.contains(&id))
				})
			}
			Some(Operation::Fetch | Operation::Find | Operation::View) => {
				uuid(&result["data"]["document_id"]).is_some_and(|id| self.documents.contains(&id))
			}
			None => false,
		}
	}

	/// Whether `event` is a research result carrying an expired Provider
	/// result. A placeholder is not one, so expiry is idempotent.
	pub fn carried_by(&self, event: &ContextEvent) -> bool {
		matches!(event, ContextEvent::Tool { result, .. } if self.covers(result))
	}
}

/// Replace every research result in `history` that carries an expired
/// Provider result with the expired placeholder, keeping its call. `originals`
/// names the entries whose original, as journaled, carries one although the
/// projection no longer shows its envelope, such as a result compaction cut
/// to a string. A revoked placeholder stays as it is. Returns whether
/// anything was replaced.
pub fn expire(
	history: &mut [HistoryEntry],
	expired: &ExpiredResults,
	originals: &BTreeSet<u64>,
) -> bool {
	let mut replaced = false;
	for entry in history {
		let carried = expired.carried_by(&entry.event) || originals.contains(&entry.seq);
		let ContextEvent::Tool { call, result } = &mut entry.event else {
			continue;
		};
		if carried
			&& !ContextEvent::is_expired_result(result)
			&& !ContextEvent::is_revoked_result(result)
		{
			*result = ContextEvent::expired_result(&call.id);
			replaced = true;
		}
	}
	replaced
}

#[cfg(test)]
mod tests;
