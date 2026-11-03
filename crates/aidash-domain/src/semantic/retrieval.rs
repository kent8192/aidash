//! Retrieval inputs and provenance use source identities held by the use case.
use super::{indexing::IndexingSpec, results::SearchResult};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImageQuery {
	pub run_id: Uuid,
	pub area_id: Uuid,
	pub generation: i64,
	pub file_id: Uuid,
	pub path: String,
	pub expected_digest: String,
}

#[derive(Debug, Clone)]
pub struct Search {
	pub query: String,
	pub image: Option<ImageQuery>,
	pub agent: Option<String>,
	pub metadata: Value,
	pub limit: usize,
	pub max_tokens: usize,
}
impl Search {
	pub fn validate(&self, spec: &IndexingSpec) -> Result<()> {
		if let Some(image) = &self.image {
			if !self.query.is_empty()
				|| image.generation <= 0
				|| image.expected_digest.is_empty()
				|| !spec
					.embedding
					.inputs
					.contains(&super::EmbeddingInputKind::Image)
			{
				return Err(Error::Invalid("invalid semantic image query".into()));
			}
			crate::registry::rules::validate_path(&image.path)?;
		} else {
			super::indexing::validate_text(&self.query, spec.max_input_bytes)?;
		}
		if self.limit == 0
			|| self.limit > spec.max_results
			|| self.max_tokens == 0
			|| self.max_tokens > spec.max_result_tokens
			|| !self.metadata.is_object()
			|| serde_json::to_vec(&self.metadata)?.len() > 4096
		{
			return Err(Error::Invalid(
				"invalid semantic search limits or filters".into(),
			));
		}
		Ok(())
	}
}

pub fn result_tokens(result: &SearchResult) -> serde_json::Result<usize> {
	// Preserve room for growth of the counter's own JSON representation.
	Ok(crate::context::estimated_tokens(&serde_json::to_string(result)?) + 16)
}
pub fn provenance(source: &Value) -> Value {
	if source["kind"] == "memory" {
		json!({"kind":"memory"})
	} else {
		source.clone()
	}
}
