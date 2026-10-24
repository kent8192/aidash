//! Compose retained-object maintenance from the native ownership repository.
use crate::{Result, store::Store};
pub(crate) async fn run(store: Store, stopping: tokio::sync::watch::Receiver<bool>) -> Result<()> {
	aidash_runtime::reclamation::run(&crate::bootstrap::reclamation_repository(&store), stopping)
		.await
		.map_err(Into::into)
}
