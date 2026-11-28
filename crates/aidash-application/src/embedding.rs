//! The embedding definition used by the Tenant's active search indexes.
use aidash_domain::semantic::{EmbeddingConfig, EmbeddingInputKind};
use async_trait::async_trait;

use crate::Result;

pub use aidash_domain::registry::EntityRef as EmbeddingRef;

pub const DEFAULT_EMBEDDING_ID: &str = "aidash.embedding";
pub const DEFAULT_EMBEDDING_VERSION: &str = "1.0.0";

pub fn default_embedding() -> EmbeddingRef {
	EmbeddingRef {
		id: DEFAULT_EMBEDDING_ID.into(),
		version: DEFAULT_EMBEDDING_VERSION.into(),
	}
}

pub fn default_embedding_config() -> EmbeddingConfig {
	EmbeddingConfig {
		provider: "openrouter".into(),
		endpoint: "https://openrouter.ai/api/v1".into(),
		provider_credential: Some("openrouter".into()),
		model: "google/gemini-embedding-2".into(),
		model_version: DEFAULT_EMBEDDING_VERSION.into(),
		dimensions: 3072,
		inputs: [EmbeddingInputKind::Text, EmbeddingInputKind::Image]
			.into_iter()
			.collect(),
	}
}

#[async_trait]
pub trait EmbeddingSettingsPort: Send + Sync {
	async fn active_embedding(&self, tenant: &str) -> Result<Option<EmbeddingRef>>;
}

/// All readers use the active version until a replacement generation is ready.
pub async fn tenant_embedding(
	settings: &dyn EmbeddingSettingsPort,
	tenant: &str,
) -> Result<EmbeddingRef> {
	Ok(settings
		.active_embedding(tenant)
		.await?
		.unwrap_or_else(default_embedding))
}
