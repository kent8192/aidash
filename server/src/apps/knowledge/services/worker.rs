//! Adapt native indexing entry points to application workflows and runtime supervision.
use crate::{Result, federation::Federation, store::Store};
use std::sync::Arc;

pub async fn run(f: Federation) -> Result<()> {
	aidash_runtime::semantic::run(
		Arc::new(crate::bootstrap::semantic_indexing_repository(&f.store)),
		Arc::new(crate::bootstrap::semantic_transport(&f.store)),
	)
	.await
	.map_err(Into::into)
}
pub async fn sweep(store: &Store) -> Result<usize> {
	aidash_application::semantic::sweep(
		&crate::bootstrap::semantic_indexing_repository(store),
		&crate::bootstrap::semantic_transport(store),
	)
	.await
	.map_err(Into::into)
}

pub async fn run_reembedding(store: Store) -> Result<()> {
	let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
	interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
	loop {
		interval.tick().await;
		if let Err(error) = Box::pin(crate::apps::knowledge::repositories::reembedding::sweep(
			&store,
		))
		.await
		{
			tracing::warn!(%error, "tenant re-embedding sweep unavailable");
		}
	}
}
