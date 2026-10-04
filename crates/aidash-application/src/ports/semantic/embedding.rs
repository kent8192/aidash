//! Embedding allowance remains charged until an approved complete response settles it.
use crate::Result;
use aidash_domain::{generation::embedding::Origin, semantic::EmbeddingConfig};
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait EmbeddingAllowance: Send {
	async fn settle(self: Box<Self>, reported: Option<u64>) -> Result<()>;
}
#[async_trait]
pub trait SemanticEmbeddingScope: Send {
	/// Reserve against the restored authority lease before provider I/O.
	/// A missing receipt means this authority has no generated ancestors.
	async fn reserve(
		&mut self,
		workspace: Uuid,
		config: &EmbeddingConfig,
		text: &str,
		origin: Origin,
	) -> Result<Option<Box<dyn EmbeddingAllowance>>>;
}
