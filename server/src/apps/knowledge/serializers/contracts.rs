use serde::{Deserialize, Serialize};
// Serializable contracts for semantic.

use chrono::{DateTime, Utc};
use reinhardt::Validate;
use schemars::JsonSchema;
use serde_json::Value;

pub use aidash_domain::semantic::{EmbeddingConfig, VectorConfig};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticIndexSpec")]
#[serde(deny_unknown_fields)]
#[derive(Validate)]
pub struct IndexSpec {
	pub embedding: EmbeddingConfig,
	pub vector: VectorConfig,
	pub enabled: bool,
	pub auto_context: bool,
	#[validate(range(min = 1, max = 1024))]
	pub max_sources: usize,
	#[validate(range(min = 1, max = 20))]
	pub max_results: usize,
	#[validate(range(min = 128, max = 32768))]
	pub max_result_tokens: usize,
	#[validate(range(min = 128, max = 32768))]
	pub max_input_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticConfigureIndex")]
#[serde(deny_unknown_fields)]
pub struct ConfigureIndex {
	pub expected_revision: i64,
	pub spec: IndexSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, JsonSchema)]
#[schemars(rename = "SemanticIndex")]
pub struct Index {
	pub workspace_id: Uuid,
	pub tenant: String,
	pub revision: i64,
	pub spec: Value,
	pub collection: String,
	pub updated_at: DateTime<Utc>,
}

pub use aidash_domain::semantic::Source;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticPutEntry")]
#[serde(deny_unknown_fields)]
pub struct PutEntry {
	/// Stable caller key; expected_revision=0 creates or replays identical input.
	pub key: String,
	pub expected_revision: i64,
	pub source: Source,
	/// Optional qualified Agent identity. Stored as a scope, never an authority claim.
	pub agent: Option<String>,
	pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, JsonSchema)]
#[schemars(rename = "SemanticEntry")]
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticRevision")]
#[serde(deny_unknown_fields)]
pub struct Revision {
	pub expected_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticSearch")]
#[serde(deny_unknown_fields)]
pub struct Search {
	pub query: String,
	pub agent: Option<String>,
	#[serde(default = "empty_object")]
	pub metadata: Value,
	pub limit: usize,
	pub max_tokens: usize,
}

#[derive(Debug, Serialize, sqlx::FromRow, JsonSchema)]
#[schemars(rename = "SemanticHistory")]
pub struct History {
	pub sequence: i64,
	pub workspace_id: Uuid,
	pub entry_id: Option<Uuid>,
	pub revision: i64,
	pub state: String,
	pub detail: String,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticCleanupCounts")]
pub struct CleanupCounts {
	pub retired: i64,
	pub pending: i64,
	pub failed: i64,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "SemanticCleanupStatus")]
pub struct CleanupStatus {
	pub points: CleanupCounts,
	pub collections: CleanupCounts,
}

pub(crate) fn empty_object() -> Value {
	serde_json::json!({})
}

use uuid::Uuid;

pub use aidash_domain::semantic::results::{Match, SearchResult};
