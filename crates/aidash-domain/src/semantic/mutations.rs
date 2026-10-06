//! Semantic entry revisions and immutable index generations are business state.
use super::{Source, indexing::IndexingSpec};
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Index {
	pub workspace_id: Uuid,
	pub tenant: String,
	pub revision: i64,
	pub spec: Value,
	pub collection: String,
	pub updated_at: DateTime<Utc>,
}
impl Index {
	pub fn configuration(&self) -> serde_json::Result<IndexingSpec> {
		serde_json::from_value(self.spec.clone())
	}
}

#[derive(Debug, Clone)]
pub struct Entry {
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub key: String,
	pub source: Value,
	pub agent: Option<String>,
	pub metadata: Value,
	pub revision: i64,
	pub point_id: Uuid,
	pub index_revision: i64,
	pub deleted: bool,
	pub state: String,
	pub attempts: i32,
	pub last_error: Option<String>,
	pub created_by: String,
	pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct History {
	pub sequence: i64,
	pub workspace_id: Uuid,
	pub entry_id: Option<Uuid>,
	pub revision: i64,
	pub state: String,
	pub detail: String,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct Put {
	pub key: String,
	pub expected_revision: i64,
	pub source: Source,
	pub agent: Option<String>,
	pub metadata: Value,
}
impl Put {
	pub fn validate(&self) -> Result<()> {
		validate_put(
			&self.key,
			self.expected_revision,
			self.agent.as_deref(),
			&self.metadata,
		)
	}
}

pub fn validate_put(key: &str, revision: i64, agent: Option<&str>, metadata: &Value) -> Result<()> {
	if key.is_empty()
		|| key.len() > 256
		|| revision < 0
		|| revision == i64::MAX
		|| !metadata.is_object()
		|| serde_json::to_vec(metadata)?.len() > 4096
		|| agent.is_some_and(|a| a.is_empty() || a.len() > 512)
	{
		return Err(Error::Invalid(
			"invalid semantic entry key, revision, scope or metadata".into(),
		));
	}
	Ok(())
}

pub fn validate_revision(revision: i64, message: &str) -> Result<()> {
	if revision < 0 || revision == i64::MAX {
		return Err(Error::Invalid(message.into()));
	}
	Ok(())
}

pub fn validate_index_limit(max: usize, largest_memory: usize) -> Result<()> {
	if largest_memory > max {
		return Err(Error::Conflict(
			"semantic index max_input_bytes is below existing memory text".into(),
		));
	}
	Ok(())
}

pub fn validate_configuration_counts(
	spec: &IndexingSpec,
	count: i64,
	largest_memory: usize,
) -> Result<()> {
	if count > spec.max_sources as i64 {
		return Err(Error::Conflict(
			"index limit is below the current source count".into(),
		));
	}
	validate_index_limit(spec.max_input_bytes, largest_memory)
}

/// Identical accepted inputs replay even after their next indexing transition.
pub fn put_replays(old: &Entry, candidate: &Entry, expected: i64) -> bool {
	(expected == 0 || old.revision == expected + 1 || old.revision == expected)
		&& old.source == candidate.source
		&& old.agent == candidate.agent
		&& old.metadata == candidate.metadata
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::rstest;
	use serde_json::json;

	#[rstest]
	#[case::empty_key("", 0, None, json!({}), false)]
	#[case::valid("key", 0, None, json!({}), true)]
	#[case::negative("key", -1, None, json!({}), false)]
	#[case::overflow("key", i64::MAX, None, json!({}), false)]
	#[case::missing_object("key", 0, None, json!(null), false)]
	#[case::array("key", 0, None, json!([]), false)]
	#[case::empty_agent("key", 0, Some(""), json!({}), false)]
	#[case::agent("key", 0, Some("aidash://node/agent@1"), json!({}), true)]
	fn entry_admission_retains_key_revision_scope_and_metadata_rules(
		#[case] key: &str,
		#[case] revision: i64,
		#[case] agent: Option<&str>,
		#[case] metadata: Value,
		#[case] allowed: bool,
	) {
		assert_eq!(
			validate_put(key, revision, agent, &metadata).is_ok(),
			allowed
		);
	}

	#[rstest]
	#[case::key(256, 0, 0, true)]
	#[case::key_overflow(257, 0, 0, false)]
	#[case::agent(1, 512, 0, true)]
	#[case::agent_overflow(1, 513, 0, false)]
	#[case::metadata_exact(1, 0, 4088, true)]
	#[case::metadata_overflow(1, 0, 4089, false)]
	fn entry_size_limits_count_the_same_serialized_bytes(
		#[case] key: usize,
		#[case] agent: usize,
		#[case] metadata: usize,
		#[case] allowed: bool,
	) {
		let agent = (agent > 0).then(|| "a".repeat(agent));
		let value = json!({"x":"a".repeat(metadata)});
		assert_eq!(
			validate_put(&"k".repeat(key), 0, agent.as_deref(), &value).is_ok(),
			allowed
		);
	}
}
