//! Compose retained-object maintenance from the native ownership repository.
use crate::{Result, store::Store};
pub async fn run(store: Store, stopping: tokio::sync::watch::Receiver<bool>) -> Result<()> {
	tokio::try_join!(
		async {
			aidash_runtime::reclamation::run(
				&crate::bootstrap::reclamation_repository(&store),
				stopping.clone(),
			)
			.await
			.map_err(crate::Error::from)
		},
		crate::apps::drives::maintenance::run(store.clone(), stopping.clone()),
	)?;
	Ok(())
}
