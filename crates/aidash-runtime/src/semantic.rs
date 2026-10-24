//! Periodic semantic indexing remains alive while execution workers drain.
use aidash_application::{
	Error, Result,
	ports::{VectorIndex, semantic::SemanticIndexingRepository},
};
use std::{sync::Arc, time::Duration};

/// The process supervisor owns and cancels this loop on shutdown.
pub async fn run(
	repository: Arc<dyn SemanticIndexingRepository>,
	vector: Arc<dyn VectorIndex>,
) -> Result<()> {
	loop {
		match aidash_application::semantic::sweep(repository.as_ref(), vector.as_ref()).await {
			Ok(_) | Err(Error::TransactionPending) => {}
			Err(error) => tracing::warn!(%error, "semantic indexing sweep failed"),
		}
		tokio::time::sleep(Duration::from_secs(1)).await;
	}
}
