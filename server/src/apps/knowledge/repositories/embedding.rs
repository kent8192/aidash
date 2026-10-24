//! Embedding authority borrows the caller's existing native scope and pool.
use crate::store::Store;
use aidash_application::{
	Result,
	ports::semantic::embedding::{EmbeddingAllowance, SemanticEmbeddingScope},
};
use aidash_domain::{generation::embedding::Origin, semantic::EmbeddingConfig};
use async_trait::async_trait;
use uuid::Uuid;

pub(crate) struct Embedding<'a, 'scope> {
	pub lease: &'a mut crate::semantic::service::Lease<'scope>,
	pub store: Store,
}
#[async_trait]
impl SemanticEmbeddingScope for Embedding<'_, '_> {
	async fn reserve(
		&mut self,
		workspace: Uuid,
		config: &EmbeddingConfig,
		text: &str,
		origin: Origin,
	) -> Result<Option<Box<dyn EmbeddingAllowance>>> {
		if let Some(access) = self.lease.access() {
			crate::generation::embedding::reserve(
				access,
				&self.store,
				workspace,
				config,
				text,
				origin,
			)
			.await
			.map(|reservation| {
				reservation.map(|receipt| Box::new(receipt) as Box<dyn EmbeddingAllowance>)
			})
			.map_err(Into::into)
		} else {
			Ok(None)
		}
	}
}
