//! Independently scheduled recovery keeps unreachable aborted history off active work.
use crate::background::Passes;
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
	let mut passes = if aborted {
		Passes::new(
			"transactions.coordinator.aborted",
			Duration::from_millis(1000),
		)
	} else {
		Passes::new(
			"transactions.coordinator.active",
			Duration::from_millis(100),
		)
	};
	loop {
		// A failed pass keeps the transaction decisions it could not act on.
		passes.run(coordinator.recover_kind(aborted)).await;
		passes.wait().await;
	}
}

/// The supervisor retains recovery during drain and cancels the owned loop afterward.
pub async fn run_participant(participant: Participant) -> Result<()> {
	let mut passes = Passes::new("transactions.participant", Duration::from_secs(1));
	loop {
		// A failed pass keeps the barriers it could not release.
		passes.run(participant.recover_once()).await;
		passes.wait().await;
	}
}
