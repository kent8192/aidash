//! Portable retrieval results preserve the existing JSON and schema names.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticMatch")]
pub struct Match {
	pub entry_id: Uuid,
	pub revision: i64,
	pub source: Value,
	pub agent: Option<String>,
	pub metadata: Value,
	pub text: String,
	pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticSearchResult")]
pub struct SearchResult {
	pub workspace_id: Uuid,
	pub index_revision: i64,
	pub model: String,
	pub model_version: String,
	pub matches: Vec<Match>,
	pub estimated_tokens: usize,
	pub truncated: bool,
}
