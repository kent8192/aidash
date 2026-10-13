//! Persistent semantic sources and vector indexes in PostgreSQL.
#[path = "backend.rs"]
pub mod backend;
#[path = "service.rs"]
pub mod service;
#[path = "worker.rs"]
pub mod worker;

use crate::{Error, Result};
use reinhardt::core::validators::Validate;
use uuid::Uuid;

/// Provider-source checks need node settings; provider invariants are pure.
pub trait EmbeddingValidation {
	fn validate(&self) -> Result<()>;
}
impl EmbeddingValidation for EmbeddingConfig {
	fn validate(&self) -> Result<()> {
		crate::bootstrap::registry_validation()
			.validate_embedding(self)
			.map_err(Into::into)
	}
}
impl IndexSpec {
	pub fn validate(&self) -> Result<()> {
		self.validate_with(&crate::bootstrap::registry_validation())
	}
	/// Validate against a node's runtime composition, so a configured Provider
	/// Credential Store admits Tenant-backed embeddings.
	pub fn validate_with(
		&self,
		validation: &aidash_application::registry::DefinitionValidation,
	) -> Result<()> {
		validation.validate_embedding(&self.embedding)?;
		if self.vector.provider != "postgres"
			|| self.vector.endpoint != "local"
			|| self.vector.credential_env.is_some()
		{
			return Err(Error::Invalid(
				"vector search requires local PostgreSQL".into(),
			));
		}
		Validate::validate(self).map_err(|error| Error::Invalid(error.to_string()))?;
		Ok(())
	}
}

impl Index {
	pub fn configuration(&self) -> Result<IndexSpec> {
		Ok(serde_json::from_value(self.spec.clone())?)
	}
}

impl PutEntry {
	pub fn validate(&self) -> Result<()> {
		aidash_domain::semantic::mutations::validate_put(
			&self.key,
			self.expected_revision,
			self.agent.as_deref(),
			&self.metadata,
		)
		.map_err(Into::into)
	}
}

pub use crate::apps::knowledge::serializers::contracts::{
	CleanupCounts, CleanupStatus, ConfigureIndex, EmbeddingConfig, Entry, History, Index,
	IndexSpec, Match, PutEntry, Revision, Search, SearchResult, Source, VectorConfig,
};
