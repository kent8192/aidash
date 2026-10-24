//! Indexing decisions use current database authority and durable point identities.
use super::{EmbeddingConfig, Source, VectorConfig};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct IndexingPlan {
	pub collection: String,
	pub tenant: String,
	pub revision: i64,
	pub spec: Value,
}

/// Decode the same persisted specification after the locked authority fence.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndexingSpec {
	pub embedding: EmbeddingConfig,
	pub vector: VectorConfig,
	pub enabled: bool,
	pub auto_context: bool,
	pub max_sources: usize,
	pub max_results: usize,
	pub max_result_tokens: usize,
	pub max_input_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct IndexingEntry {
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub source: Value,
	pub revision: i64,
	pub point_id: Uuid,
	pub state: String,
	pub attempts: i32,
}

pub fn source(entry: &IndexingEntry) -> serde_json::Result<Source> {
	serde_json::from_value(entry.source.clone())
}

pub fn validate_text(text: &str, max: usize) -> Result<()> {
	if text.trim().is_empty() || text.len() > max {
		Err(Error::Invalid(
			"semantic text is empty or exceeds the configured byte limit".into(),
		))
	} else {
		Ok(())
	}
}

pub fn retry(attempts: i32) -> (i32, f64) {
	let attempts = attempts.saturating_add(1);
	let delay = 2_i64.pow(attempts.min(8) as u32).min(300);
	(attempts, delay as f64)
}

pub fn content_digest(text: &str) -> String {
	use sha2::{Digest, Sha256};
	format!("{:x}", Sha256::digest(text.as_bytes()))
}

pub fn needs_new_point(old: &Option<(Option<String>, bool)>, digest: &str) -> bool {
	old.as_ref().is_some_and(|(old_digest, retired)| {
		*retired || old_digest.as_deref().is_some_and(|old| old != digest)
	})
}

pub fn has_current_point(
	entry: &IndexingEntry,
	old: &Option<(Option<String>, bool)>,
	digest: &str,
) -> bool {
	entry.state == "READY"
		&& old
			.as_ref()
			.is_some_and(|(old_digest, retired)| old_digest.as_deref() == Some(digest) && !*retired)
}

#[cfg(test)]
mod tests;
