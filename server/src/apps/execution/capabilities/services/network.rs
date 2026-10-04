//! Compose the outbound worker from shared authority persistence and the external HTTP adapter.
use crate::{Result, store::Store};
pub(crate) async fn run(store: Store, stopping: tokio::sync::watch::Receiver<bool>) -> Result<()> {
	aidash_runtime::outbound::run(
		&crate::bootstrap::outbound_repository(&store),
		&crate::bootstrap::outbound_transport(),
		stopping,
	)
	.await
	.map_err(Into::into)
}
