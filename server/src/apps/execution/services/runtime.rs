//! Compatibility entry points compose native ports with the shared worker driver.
use crate::{Result, domain::Run, federation::Federation};
use aidash_application::ports::execution::InferenceInterruption;
use aidash_domain::RunControl;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct Harness {
	pub federation: Federation,
}
impl Harness {
	pub async fn worker_once(&self) -> Result<bool> {
		let leases = crate::bootstrap::worker_leases(&self.federation.store);
		let terminal = crate::bootstrap::terminal_repository(&self.federation);
		if aidash_harness::execution::failure_once(
			&terminal,
			&leases,
			self.federation.config.lease_seconds,
		)
		.await?
		{
			return Ok(true);
		}
		if aidash_harness::execution::terminal::pending(&terminal).await? {
			return Ok(true);
		}
		let visibility =
			crate::transactions::gate::ReadLease::begin(&self.federation.store).await?;
		let token = Uuid::new_v4();
		let Some(run) = self
			.federation
			.store
			.lease_run(token, self.federation.config.lease_seconds)
			.await?
		else {
			return Ok(false);
		};
		self.advance_leased(run, token, visibility).await
	}

	/// The caller has committed the lease and, for notifications, its disposition.
	pub(crate) async fn advance_leased(
		&self,
		run: Run,
		token: Uuid,
		visibility: crate::transactions::gate::ReadLease,
	) -> Result<bool> {
		let scope = crate::bootstrap::worker_step(&self.federation, run, visibility);
		let leases = crate::bootstrap::worker_leases(&self.federation.store);
		aidash_harness::execution::advance(
			scope,
			&leases,
			token,
			self.federation.config.lease_seconds,
		)
		.await?;
		Ok(true)
	}

	pub async fn deliver_terminal_messages_until(
		&self,
		stopping: tokio::sync::watch::Receiver<bool>,
	) -> Result<()> {
		aidash_harness::execution::deliver_terminal_until(
			Arc::new(crate::bootstrap::terminal_repository(&self.federation)),
			Arc::new(crate::bootstrap::worker_leases(&self.federation.store)),
			self.federation.config.lease_seconds,
			stopping,
		)
		.await
		.map_err(Into::into)
	}

	pub async fn run_worker(&self) -> Result<()> {
		let (_sender, receiver) = tokio::sync::watch::channel(false);
		self.run_worker_until(receiver).await
	}

	/// Finish the current durable step, then stop claiming work on shutdown.
	pub async fn run_worker_until(
		&self,
		stopping: tokio::sync::watch::Receiver<bool>,
	) -> Result<()> {
		let activation = crate::bootstrap::activation_driver(
			self.federation.clone(),
			crate::activation::Settings::from_env()?,
			true,
		);
		aidash_harness::execution::run_worker(
			activation,
			Arc::new(crate::bootstrap::activation_repository(self.clone())),
			Arc::new(crate::bootstrap::terminal_repository(&self.federation)),
			Arc::new(crate::bootstrap::worker_leases(&self.federation.store)),
			self.federation.config.lease_seconds,
			stopping,
		)
		.await
		.map_err(Into::into)
	}
}

/// Resolve when committed Run control is Cancelled or a Run input newer than
/// the request's included input is committed. Reads happen outside the worker's
/// authority transaction, and a failed observation never interrupts inference.
pub(crate) async fn wait_for_inference_interruption(
	store: &crate::store::Store,
	id: Uuid,
	included_input_seq: i64,
) -> Result<InferenceInterruption> {
	loop {
		match inference_interruption(store, id, included_input_seq).await {
			Ok(Some(interruption)) => return Ok(interruption),
			Ok(None) => {}
			Err(error) => {
				tracing::warn!(run_id = %id, %error, "inference interruption poll failed; retrying")
			}
		}
		tokio::time::sleep(INTERRUPTION_POLL).await;
	}
}

const INTERRUPTION_POLL: std::time::Duration = std::time::Duration::from_millis(250);

async fn inference_interruption(
	store: &crate::store::Store,
	id: Uuid,
	included_input_seq: i64,
) -> Result<Option<InferenceInterruption>> {
	use crate::apps::execution::models::{Run as RunRecord, RunInput};
	let lease = store.orm_connection()?;
	if RunRecord::committed_control(&mut lease.handle(), id).await? == RunControl::Cancelled {
		return Ok(Some(InferenceInterruption::Cancelled));
	}
	let mut tx = store.database().begin().await?;
	let newer = RunInput::newer_than(tx.as_mut(), id, included_input_seq).await?;
	tx.commit().await?;
	Ok(newer.then_some(InferenceInterruption::Superseded))
}

#[cfg(test)]
fn retryable_inference_error(error: &crate::Error) -> bool {
	crate::apps::execution::repositories::worker::classify_failure(error).retryable()
}

#[cfg(test)]
#[path = "../tests/services_runtime_review_tests.rs"]
mod review_tests;
