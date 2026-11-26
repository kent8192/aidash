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
	#[serde(default = "aidash_domain::semantic::indexing::default_max_image_bytes")]
	#[validate(range(min = 1, max = 67108864))]
	pub max_image_bytes: usize,
	#[serde(default = "aidash_domain::semantic::indexing::default_max_images")]
	#[validate(range(max = 1024))]
	pub max_images: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticConfigureIndex")]
#[serde(deny_unknown_fields)]
pub struct ConfigureIndex {
	pub expected_revision: i64,
	/// Confirms that disabling the index or changing its embedding deletes or
	/// recomputes the embeddings of this Thread's Built-in Memory facts (#1123).
	#[serde(default)]
	pub acknowledge_memory_embeddings: bool,
	pub spec: IndexSpec,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticConfigureIndex")]
#[serde(deny_unknown_fields)]
pub struct ConfigureThreadIndex {
	pub expected_revision: i64,
	/// Confirms deleting this Thread's memory embeddings when disabling its index.
	#[serde(default)]
	pub acknowledge_memory_embeddings: bool,
	pub spec: ThreadIndexOptions,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticThreadIndexOptions")]
#[serde(deny_unknown_fields)]
pub struct ThreadIndexOptions {
	pub vector: VectorConfig,
	pub enabled: bool,
	pub auto_context: bool,
	pub max_sources: usize,
	pub max_results: usize,
	pub max_result_tokens: usize,
	pub max_input_bytes: usize,
	#[serde(default = "aidash_domain::semantic::indexing::default_max_image_bytes")]
	pub max_image_bytes: usize,
	#[serde(default = "aidash_domain::semantic::indexing::default_max_images")]
	pub max_images: usize,
}

impl From<ThreadIndexOptions> for aidash_domain::semantic::indexing::IndexingSpec<()> {
	fn from(options: ThreadIndexOptions) -> Self {
		Self {
			embedding: (),
			vector: options.vector,
			enabled: options.enabled,
			auto_context: options.auto_context,
			max_sources: options.max_sources,
			max_results: options.max_results,
			max_result_tokens: options.max_result_tokens,
			max_input_bytes: options.max_input_bytes,
			max_image_bytes: options.max_image_bytes,
			max_images: options.max_images,
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticIndex")]
pub struct Index {
	pub thread_id: Uuid,
	pub tenant: String,
	pub revision: i64,
	pub spec: Value,
	pub collection: String,
	pub updated_at: DateTime<Utc>,
}
crate::native_record!(Index {
	thread_id,
	tenant,
	revision,
	spec,
	collection,
	updated_at
});

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

/// The entries a reader may see, with the entries a Node's silence left out (#770).
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[schemars(rename = "SemanticEntryList")]
pub struct EntryList {
	pub entries: Vec<Entry>,
	/// Entries this read scanned and could not confirm because a Node did not
	/// answer, for each Node. `[]` when none.
	pub unconfirmed: Vec<aidash_domain::unconfirmed::Unconfirmed>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticEntry")]
pub struct Entry {
	pub id: Uuid,
	pub thread_id: Uuid,
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
crate::native_record!(Entry {
	id,
	thread_id,
	key,
	source,
	agent,
	metadata,
	revision,
	point_id,
	index_revision,
	deleted,
	state,
	attempts,
	last_error,
	created_by,
	updated_at
});

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
	#[serde(default)]
	pub query: String,
	#[serde(default)]
	pub image: Option<aidash_domain::semantic::retrieval::ImageQuery>,
	/// Set only by a Run's own `workspace_search`; never accepted over HTTP.
	#[serde(skip)]
	#[schemars(skip)]
	pub attachment: Option<aidash_domain::semantic::retrieval::AttachmentImageQuery>,
	pub agent: Option<String>,
	#[serde(default = "empty_object")]
	pub metadata: Value,
	pub limit: usize,
	pub max_tokens: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "SemanticHistory")]
pub struct History {
	pub sequence: i64,
	pub thread_id: Uuid,
	pub entry_id: Option<Uuid>,
	pub revision: i64,
	pub state: String,
	pub detail: String,
	pub created_at: DateTime<Utc>,
}
crate::native_record!(History {
	sequence,
	thread_id,
	entry_id,
	revision,
	state,
	detail,
	created_at
});

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
