//! Independently scheduled recovery keeps unreachable aborted history off active work.
use aidash_application::{Result, transactions::coordination::Coordinator};
use std::time::Duration;

/// Bootstrap supplies separate storage capacity for active and aborted work.
/// Both loops remain alive while workers drain; the supervisor owns cancellation.
pub async fn run_coordinator(active: Coordinator, aborted: Coordinator) -> Result<()> {
	tokio::try_join!(recover(active, false), recover(aborted, true))?;
	Ok(())
}

async fn recover(coordinator: Coordinator, aborted: bool) -> Result<()> {
	loop {
		if let Err(error) = coordinator.recover_kind(aborted).await {
			tracing::warn!(%error, aborted, "coordinator recovery failed; decisions retained");
		}
		tokio::time::sleep(Duration::from_millis(if aborted { 1000 } else { 100 })).await;
	}
}
