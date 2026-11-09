//! The API, probe and metrics listeners outlive a failed accept the way the
//! gateways do (#779): Reinhardt waits and accepts again, and this handler
//! counts and logs the failures as the gateways do.
//!
//! A probe listener that is out of descriptors does not answer `/live`, so a
//! shortage that lasts past the liveness probe's failure threshold still ends
//! in a restart, which also clears a descriptor leak. A shorter one does not.
use aidash_integrations::accept::{AcceptFailures, RETRY_AFTER};
use reinhardt::server::{AcceptErrorAction, AcceptErrorHandler};
use std::{io, sync::Arc, sync::Mutex};

pub(crate) struct ListenerAcceptFailures(Mutex<AcceptFailures>);

impl ListenerAcceptFailures {
	/// `service` names the listener in the failure counter and the logs.
	pub(crate) fn new(service: &'static str) -> Arc<Self> {
		Arc::new(Self(Mutex::new(AcceptFailures::new(service))))
	}

	fn failures(&self) -> std::sync::MutexGuard<'_, AcceptFailures> {
		// The state only feeds logs and a counter, so a poisoned lock is still usable.
		self.0
			.lock()
			.unwrap_or_else(|poisoned| poisoned.into_inner())
	}
}

impl AcceptErrorHandler for ListenerAcceptFailures {
	fn on_error(&self, error: &io::Error) -> AcceptErrorAction {
		self.failures().failed(error);
		AcceptErrorAction::RetryAfter(RETRY_AFTER)
	}

	fn on_accepted(&self) {
		self.failures().accepted();
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::rstest;

	#[rstest]
	#[tokio::test(start_paused = true)]
	async fn every_failure_waits_a_second_and_the_listener_keeps_going() {
		// Arrange
		let handler = ListenerAcceptFailures::new("test listener");
		let error = io::Error::from_raw_os_error(24);

		// Act
		let first = handler.on_error(&error);
		handler.on_accepted();
		let second = handler.on_error(&error);

		// Assert
		assert_eq!(first, AcceptErrorAction::RetryAfter(RETRY_AFTER));
		assert_eq!(second, AcceptErrorAction::RetryAfter(RETRY_AFTER));
	}
}
