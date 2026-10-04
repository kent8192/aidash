//! Compatibility entry points compose native ports with the shared worker driver.
use crate::{Result, domain::Run, federation::Federation};
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
		if aidash_runtime::execution::failure_once(
			&terminal,
			&leases,
			self.federation.config.lease_seconds,
		)
		.await?
		{
			return Ok(true);
		}
		if aidash_application::execution::terminal::pending(&terminal).await? {
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
		aidash_runtime::execution::advance(
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
		aidash_runtime::execution::deliver_terminal_until(
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
		aidash_runtime::execution::run_worker(
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

pub(crate) async fn wait_for_inference_cancellation(
	store: &crate::store::Store,
	id: Uuid,
) -> Result<()> {
	aidash_runtime::execution::wait_for_cancellation(&crate::bootstrap::worker_leases(store), id)
		.await
		.map_err(Into::into)
}

#[cfg(test)]
fn retryable_inference_error(error: &crate::Error) -> bool {
	crate::apps::execution::repositories::worker::classify_failure(error).retryable()
}

#[cfg(test)]
#[path = "../tests/services_runtime_review_tests.rs"]
mod review_tests;
