//! Persistent semantic sources with PostgreSQL authority and Qdrant indexes.
pub mod api;
pub mod backend;
pub mod service;
pub mod worker;

use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = SemanticEmbeddingConfig)]
pub struct EmbeddingConfig {
	pub provider: String,
	pub endpoint: String,
	pub credential_env: Option<String>,
	pub model: String,
	pub model_version: String,
	pub dimensions: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = SemanticVectorConfig)]
pub struct VectorConfig {
	pub provider: String,
	pub endpoint: String,
	pub credential_env: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = SemanticIndexSpec)]
pub struct IndexSpec {
	pub embedding: EmbeddingConfig,
	pub vector: VectorConfig,
	pub enabled: bool,
	pub auto_context: bool,
	pub max_sources: usize,
	pub max_results: usize,
	pub max_result_tokens: usize,
	pub max_input_bytes: usize,
}
impl EmbeddingConfig {
	pub fn validate(&self) -> Result<()> {
		self.validate_in(true)
	}
	pub(crate) fn validate_in(&self, local: bool) -> Result<()> {
		crate::config::validate_endpoint(&self.endpoint)?;
		if let Some(name) = &self.credential_env {
			crate::config::validate_secret_reference(name)?;
			if local {
				crate::config::secret(name)?;
			}
		}
		if self.provider != "openai"
			|| self.model.trim().is_empty()
			|| self.model.len() > 256
			|| self.model_version.trim().is_empty()
			|| self.model_version.len() > 128
			|| !(1..=8192).contains(&self.dimensions)
		{
			return Err(Error::Invalid(
				"invalid embedding provider, model, version or dimensions".into(),
			));
		}
		Ok(())
	}
}
impl IndexSpec {
	pub fn validate(&self) -> Result<()> {
		self.embedding.validate()?;
		if self.vector.provider != "qdrant" {
			return Err(Error::Invalid("supported vector provider is qdrant".into()));
		}
		crate::config::validate_endpoint(&self.vector.endpoint)?;
		if let Some(name) = &self.vector.credential_env {
			crate::config::secret(name)?;
		}
		if !(1..=1024).contains(&self.max_sources)
			|| !(1..=20).contains(&self.max_results)
			|| !(128..=32768).contains(&self.max_result_tokens)
			|| !(128..=32768).contains(&self.max_input_bytes)
		{
			return Err(Error::Invalid("invalid semantic resource limits".into()));
		}
		Ok(())
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = SemanticConfigureIndex)]
pub struct ConfigureIndex {
	pub expected_revision: i64,
	pub spec: IndexSpec,
}
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
#[schema(as = SemanticIndex)]
pub struct Index {
	pub workspace_id: Uuid,
	pub tenant: String,
	pub revision: i64,
	pub spec: Value,
	pub collection: String,
	pub updated_at: DateTime<Utc>,
}
impl Index {
	pub fn configuration(&self) -> Result<IndexSpec> {
		Ok(serde_json::from_value(self.spec.clone())?)
	}
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[schema(as = SemanticSource)]
pub enum Source {
	Memory { text: String },
	Artifact { id: Uuid },
	Message { id: Uuid },
}
impl Source {
	pub fn same_origin(&self, other: &Self) -> bool {
		match (self, other) {
			(Self::Memory { .. }, Self::Memory { .. }) => true,
			_ => self == other,
		}
	}
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = SemanticPutEntry)]
pub struct PutEntry {
	/// Stable caller key; expected_revision=0 creates or replays identical input.
	pub key: String,
	pub expected_revision: i64,
	pub source: Source,
	/// Optional qualified Agent identity. Stored as a scope, never an authority claim.
	pub agent: Option<String>,
	pub metadata: Value,
}
impl PutEntry {
	pub fn validate(&self) -> Result<()> {
		if self.key.is_empty()
			|| self.key.len() > 256
			|| self.expected_revision < 0
			|| self.expected_revision == i64::MAX
			|| !self.metadata.is_object()
			|| serde_json::to_vec(&self.metadata)?.len() > 4096
			|| self
				.agent
				.as_ref()
				.is_some_and(|a| a.is_empty() || a.len() > 512)
		{
			return Err(Error::Invalid(
				"invalid semantic entry key, revision, scope or metadata".into(),
			));
		}
		Ok(())
	}
}
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
#[schema(as = SemanticEntry)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = SemanticRevision)]
pub struct Revision {
	pub expected_revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = SemanticSearch)]
pub struct Search {
	pub query: String,
	pub agent: Option<String>,
	#[serde(default = "empty_object")]
	pub metadata: Value,
	pub limit: usize,
	pub max_tokens: usize,
}
fn empty_object() -> Value {
	serde_json::json!({})
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = SemanticMatch)]
pub struct Match {
	pub entry_id: Uuid,
	pub revision: i64,
	pub source: Value,
	pub agent: Option<String>,
	pub metadata: Value,
	pub text: String,
	pub score: f32,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = SemanticSearchResult)]
pub struct SearchResult {
	pub workspace_id: Uuid,
	pub index_revision: i64,
	pub model: String,
	pub model_version: String,
	pub matches: Vec<Match>,
	pub estimated_tokens: usize,
	pub truncated: bool,
}
#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
#[schema(as = SemanticHistory)]
pub struct History {
	pub sequence: i64,
	pub workspace_id: Uuid,
	pub entry_id: Option<Uuid>,
	pub revision: i64,
	pub state: String,
	pub detail: String,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
#[schema(as = SemanticCleanupCounts)]
pub struct CleanupCounts {
	pub retired: i64,
	pub pending: i64,
	pub failed: i64,
}
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[schema(as = SemanticCleanupStatus)]
pub struct CleanupStatus {
	pub points: CleanupCounts,
	pub collections: CleanupCounts,
}
