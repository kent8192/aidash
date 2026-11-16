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

/// A Thread Attachment image used as a query. Only an attachment the named Run
/// has already selected for its own inference qualifies (#622 decision 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentImageQuery {
	pub run_id: Uuid,
	pub message_id: Uuid,
	pub attachment_id: Uuid,
	pub expected_digest: String,
}

#[derive(Debug, Clone)]
pub struct Search {
	pub query: String,
	pub image: Option<ImageQuery>,
	pub attachment: Option<AttachmentImageQuery>,
	pub agent: Option<String>,
	pub metadata: Value,
	pub limit: usize,
	pub max_tokens: usize,
}
impl Search {
	pub fn validate(&self, spec: &IndexingSpec) -> Result<()> {
		if let Some(attachment) = &self.attachment {
			if !self.query.is_empty()
				|| self.image.is_some()
				|| attachment.expected_digest.is_empty()
				|| !spec
					.embedding
					.inputs
					.contains(&super::EmbeddingInputKind::Image)
			{
				return Err(Error::Invalid("invalid semantic image query".into()));
			}
		} else if let Some(image) = &self.image {
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
	// Preserve room for growth of the counter's own JSON representation. An
	// empty `unconfirmed` is not counted, so an ordinary result keeps its
	// estimate; a non-empty one is counted as an Agent receives it, with its
	// note (#770).
	let mut value = serde_json::to_value(result)?;
	crate::unconfirmed::for_tool(&mut value);
	Ok(crate::context::estimated_tokens(&value.to_string()) + 16)
}
pub fn provenance(source: &Value) -> Value {
	if source["kind"] == "memory" {
		json!({"kind":"memory"})
	} else {
		source.clone()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn result(unconfirmed: Vec<crate::unconfirmed::Unconfirmed>) -> SearchResult {
		SearchResult {
			workspace_id: Uuid::from_u128(1),
			index_revision: 1,
			model: "embedding".into(),
			model_version: "1".into(),
			matches: vec![],
			estimated_tokens: 0,
			truncated: false,
			unconfirmed,
		}
	}

	#[test]
	fn the_estimate_covers_the_note_an_agent_receives_with_unconfirmed_entries() {
		let entry = crate::unconfirmed::Unconfirmed {
			node: "aidash://local/brave-otter-k3m9q2xa".into(),
			kind: crate::unconfirmed::Kind::SemanticEntry,
			workspace_id: Uuid::from_u128(1),
			count: 2,
		};
		let left_out = result(vec![entry]);
		let mut delivered = serde_json::to_value(&left_out).unwrap();
		crate::unconfirmed::for_tool(&mut delivered);
		assert!(delivered.get("unconfirmed_note").is_some());
		assert!(
			result_tokens(&left_out).unwrap()
				>= crate::context::estimated_tokens(&delivered.to_string()) + 16
		);
		// An ordinary result is estimated without the field.
		let ordinary = result(vec![]);
		let mut bare = serde_json::to_value(&ordinary).unwrap();
		bare.as_object_mut().unwrap().remove("unconfirmed");
		assert_eq!(
			result_tokens(&ordinary).unwrap(),
			crate::context::estimated_tokens(&bare.to_string()) + 16
		);
	}
}
