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
