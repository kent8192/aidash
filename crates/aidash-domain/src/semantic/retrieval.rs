//! Retrieval inputs and provenance use source identities held by the use case.
use super::{indexing::IndexingSpec, results::SearchResult};
use crate::{Error, Result};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct Search {
	pub query: String,
	pub agent: Option<String>,
	pub metadata: Value,
	pub limit: usize,
	pub max_tokens: usize,
}
impl Search {
	pub fn validate(&self, spec: &IndexingSpec) -> Result<()> {
		super::indexing::validate_text(&self.query, spec.max_input_bytes)?;
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
