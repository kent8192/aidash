//! Independently scheduled recovery keeps unreachable aborted history off active work.
use aidash_application::{
	Result,
	transactions::{coordination::Coordinator, participation::Participant},
};
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

/// The supervisor retains recovery during drain and cancels the owned loop afterward.
pub async fn run_participant(participant: Participant) -> Result<()> {
	loop {
		if let Err(error) = participant.recover_once().await {
			tracing::warn!(%error, "participant recovery failed; barriers retained");
		}
		tokio::time::sleep(Duration::from_secs(1)).await;
	}
}
