//! Persistent semantic sources with PostgreSQL authority and Qdrant indexes.
#[path = "backend.rs"]
pub mod backend;
#[path = "service.rs"]
pub mod service;
#[path = "worker.rs"]
pub mod worker;

use crate::{Error, Result};
use reinhardt::core::validators::Validate;
use uuid::Uuid;

/// Local credential validation is an adapter concern; provider invariants are pure.
pub trait EmbeddingValidation {
	fn validate(&self) -> Result<()>;
	fn validate_in(&self, local: bool) -> Result<()>;
}
impl EmbeddingValidation for EmbeddingConfig {
	fn validate(&self) -> Result<()> {
		self.validate_in(true)
	}
	fn validate_in(&self, local: bool) -> Result<()> {
		crate::bootstrap::registry_validation()
			.validate_embedding(self, local)
			.map_err(Into::into)
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
