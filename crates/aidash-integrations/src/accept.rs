//! A listener outlives a failed accept: it logs, waits and tries again. The
//! gateways do this in their own accept loops (#743), and the server's API,
//! probe and metrics listeners through Reinhardt's accept error handler (#779).
//! Running out of file descriptors is the usual cause, and connections closing
//! ends it, so no kind of error stops a listener. A TCP probe can stay green
//! meanwhile, because the kernel completes handshakes until its queue is full.
use std::{io, time::Duration};
use tokio::{sync::watch, time::Instant};

/// The wait between a failed accept and the next try.
pub const RETRY_AFTER: Duration = Duration::from_secs(1);
const FAILURE_LOG_EVERY: Duration = Duration::from_secs(60);
/// One accepted connection does not end a shortage: a descriptor may just have
/// come free. Only this long without a failure does.
const RECOVERED_AFTER: Duration = Duration::from_secs(60);

/// The state one listener keeps between accepts: whether they are failing, and for how long.
pub struct AcceptFailures {
	service: &'static str,
	failing: Option<Failing>,
}

struct Failing {
	since: Instant,
	last: Instant,
	count: u64,
	logged: Instant,
}

impl AcceptFailures {
	pub fn new(service: &'static str) -> Self {
		Self {
			service,
			failing: None,
		}
	}

	/// Hand back an accepted connection. After a failure, record it, wait until
	/// the next try or until the gateway is asked to stop, and hand back nothing.
	pub(crate) async fn settle<T>(
		&mut self,
		accepted: io::Result<T>,
		stopping: &mut watch::Receiver<bool>,
	) -> Option<T> {
		match accepted {
			Ok(accepted) => {
				self.accepted();
				Some(accepted)
			}
			Err(error) => {
				self.failed(&error);
				tokio::select! {
					_ = tokio::time::sleep(RETRY_AFTER) => {},
					_ = stopping.changed() => {},
				}
				None
			}
		}
	}

	/// Record an accepted connection, for a listener that waits elsewhere.
	pub fn accepted(&mut self) {
		self.end_failure_if_recovered();
	}

	/// Record a failed accept, for a listener that waits elsewhere. It should
	/// wait [`RETRY_AFTER`] before the next try.
	pub fn failed(&mut self, error: &io::Error) {
		self.end_failure_if_recovered();
		self.record_failure(error);
	}

	/// This runs on the next accept, so a quiet listener reports recovery late, but
	/// the seconds it reports still end at the last failure.
	fn end_failure_if_recovered(&mut self) {
		if let Some(failing) = self
			.failing
			.take_if(|failing| failing.last.elapsed() >= RECOVERED_AFTER)
		{
			tracing::info!(
				service = self.service,
				failures = failing.count,
				seconds = failing.last.duration_since(failing.since).as_secs(),
				"accepting connections recovered"
			);
		}
	}

	fn record_failure(&mut self, error: &io::Error) {
		metrics::counter!("aidash_background_service_failures_total", "service" => self.service)
			.increment(1);
		let now = Instant::now();
		let failing = self.failing.get_or_insert(Failing {
			since: now,
			last: now,
			count: 0,
			logged: now,
		});
		failing.last = now;
		failing.count += 1;
		if failing.count == 1 {
			tracing::warn!(service = self.service, %error, "accepting a connection failed; retrying");
		} else if now.duration_since(failing.logged) >= FAILURE_LOG_EVERY {
			failing.logged = now;
			tracing::warn!(
				service = self.service,
				failures = failing.count,
				seconds = now.duration_since(failing.since).as_secs(),
				%error,
				"accepting connections is still failing; retrying"
			);
		}
	}
}

#[cfg(test)]
mod tests;
