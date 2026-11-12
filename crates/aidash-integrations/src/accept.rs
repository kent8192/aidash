//! A gateway outlives a failed accept: it logs, waits and tries again (#743).
//! Running out of file descriptors is the usual cause, and connections closing
//! ends it, so no kind of error stops the gateway. A TCP probe can stay green
//! meanwhile, because the kernel completes handshakes until its queue is full.
use std::{io, time::Duration};
use tokio::{sync::watch, time::Instant};

const RETRY_AFTER: Duration = Duration::from_secs(1);
const FAILURE_LOG_EVERY: Duration = Duration::from_secs(60);
/// One accepted connection does not end a shortage: a descriptor may just have
/// come free. Only this long without a failure does.
const RECOVERED_AFTER: Duration = Duration::from_secs(60);

/// The state one gateway keeps between accepts: whether they are failing, and for how long.
pub(crate) struct AcceptFailures {
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
	pub(crate) fn new(service: &'static str) -> Self {
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
		self.end_failure_if_recovered();
		match accepted {
			Ok(accepted) => Some(accepted),
			Err(error) => {
				self.record_failure(&error);
				tokio::select! {
					_ = tokio::time::sleep(RETRY_AFTER) => {},
					_ = stopping.changed() => {},
				}
				None
			}
		}
	}

	/// This runs on the next accept, so a quiet gateway reports recovery late, but
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
