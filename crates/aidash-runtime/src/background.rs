//! A background loop outlives every failed pass: it logs, waits longer and tries again.
use aidash_application::Result;
use std::{future::Future, pin::pin, time::Duration};
use tokio::{sync::watch, time::Instant};

/// Jitter is included in this cap, as in worker activation.
const MAX_DELAY: Duration = Duration::from_secs(5);
const FAILURE_LOG_EVERY: Duration = Duration::from_secs(60);
const SLOW_PASS_LOG_EVERY: Duration = Duration::from_secs(300);
const PASS_AGE_EVERY: Duration = Duration::from_secs(30);

/// The state one loop keeps between passes: whether it is failing, and for how long.
pub struct Passes {
	service: &'static str,
	interval: Duration,
	failing: Option<Failing>,
}

struct Failing {
	since: Instant,
	passes: u64,
	logged: Instant,
	backoff: Duration,
}

impl Passes {
	pub fn new(service: &'static str, interval: Duration) -> Self {
		metrics::gauge!("aidash_background_service_pass_age_seconds", "service" => service)
			.set(0.0);
		Self {
			service,
			interval,
			failing: None,
		}
	}

	/// Run one pass and record its outcome. A slow pass is reported, never cancelled:
	/// a pass may legitimately hold a long Runner call or an unbounded batch.
	pub async fn run<T>(&mut self, pass: impl Future<Output = Result<T>>) {
		let result = self.watched(pass).await;
		self.record(result.map(drop));
	}

	/// Wait for the next pass, or until the loop is asked to stop.
	pub async fn pause(&self, stopping: &mut watch::Receiver<bool>) {
		tokio::select! {_=stopping.changed()=>{},_=tokio::time::sleep(self.delay())=>{}}
	}

	/// Wait for the next pass of a loop that the Supervisor cancels instead of stopping.
	pub async fn wait(&self) {
		tokio::time::sleep(self.delay()).await;
	}

	async fn watched<T>(&self, pass: impl Future<Output = Result<T>>) -> Result<T> {
		let started = Instant::now();
		let mut pass = pin!(pass);
		let mut age = tokio::time::interval_at(started + PASS_AGE_EVERY, PASS_AGE_EVERY);
		let mut next_log = SLOW_PASS_LOG_EVERY;
		let result = loop {
			tokio::select! {
				result = &mut pass => break result,
				_ = age.tick() => {
					let elapsed = started.elapsed();
					self.set_pass_age(elapsed.as_secs_f64());
					if elapsed >= next_log {
						next_log += SLOW_PASS_LOG_EVERY;
						tracing::warn!(
							service = self.service,
							seconds = elapsed.as_secs(),
							"background service pass has not finished"
						);
					}
				}
			}
		};
		if started.elapsed() >= PASS_AGE_EVERY {
			self.set_pass_age(0.0);
		}
		result
	}

	fn set_pass_age(&self, seconds: f64) {
		metrics::gauge!("aidash_background_service_pass_age_seconds", "service" => self.service)
			.set(seconds);
	}

	fn record(&mut self, result: Result<()>) {
		let now = Instant::now();
		match result {
			Ok(()) => {
				if let Some(failing) = self.failing.take() {
					tracing::info!(
						service = self.service,
						failures = failing.passes,
						seconds = now.duration_since(failing.since).as_secs(),
						"background service recovered"
					);
				}
			}
			Err(error) => {
				metrics::counter!("aidash_background_service_failures_total", "service" => self.service)
					.increment(1);
				let interval = self.interval;
				let failing = self.failing.get_or_insert(Failing {
					since: now,
					passes: 0,
					logged: now,
					backoff: interval,
				});
				failing.passes += 1;
				failing.backoff = (failing.backoff * 2).min(MAX_DELAY);
				if failing.passes == 1 {
					tracing::warn!(service = self.service, %error, "background service pass failed; retrying");
				} else if now.duration_since(failing.logged) >= FAILURE_LOG_EVERY {
					failing.logged = now;
					tracing::warn!(
						service = self.service,
						failures = failing.passes,
						seconds = now.duration_since(failing.since).as_secs(),
						%error,
						"background service is still failing; retrying"
					);
				}
			}
		}
	}

	fn delay(&self) -> Duration {
		match &self.failing {
			None => self.interval,
			Some(failing) => {
				let jitter = u64::from(uuid::Uuid::new_v4().as_bytes()[0]) % 101;
				(failing.backoff + Duration::from_millis(jitter)).min(MAX_DELAY)
			}
		}
	}
}

#[cfg(test)]
mod tests;
