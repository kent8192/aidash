//! Supervised activation publication, finite worker pulls and database recovery.
use aidash_application::{
	Result,
	activation::{self, Publication},
	ports::activation::*,
};
use std::{
	path::PathBuf,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
	time::Duration,
};
use tokio::{sync::watch, time::Instant};
#[derive(Clone, Debug)]
pub struct Settings {
	pub recovery: Duration,
	pub fallback: Duration,
	pub test_pause_file: Option<PathBuf>,
	pub test_after_ack_pause_file: Option<PathBuf>,
}
pub struct Runtime {
	repository: Arc<dyn ActivationRepository>,
	connector: Arc<dyn ActivationConnector>,
	settings: Settings,
	worker: bool,
	broker: watch::Sender<Option<Arc<dyn ActivationTransport>>>,
	failed: AtomicBool,
	progress: AtomicBool,
}
impl Runtime {
	pub fn new(
		repository: Arc<dyn ActivationRepository>,
		connector: Arc<dyn ActivationConnector>,
		settings: Settings,
		worker: bool,
	) -> Arc<Self> {
		let (broker, _) = watch::channel(None);
		Arc::new(Self {
			repository,
			connector,
			settings,
			worker,
			broker,
			failed: AtomicBool::new(false),
			progress: AtomicBool::new(false),
		})
	}
	/// A separate supervised task: connection/setup deadlines cannot hold worker slots.
	pub async fn run(self: Arc<Self>, mut stopping: watch::Receiver<bool>) -> Result<()> {
		let mut backoff = Duration::from_millis(500);
		let mut state = "recovering";
		self.mode(state);
		while !*stopping.borrow() {
			let setup = async {
				loop {
					let reconciled = self.repository.reconcile().await?;
					if reconciled < activation::RECONCILE_BATCH_SIZE {
						break;
					}
				}
				self.connector.connect().await
			};
			let result = tokio::select! {
				result = setup => result,
				_ = stopped(&mut stopping) => break,
			};
			match result {
				Ok(broker) => {
					backoff = Duration::from_millis(500);
					self.failed.store(false, Ordering::Release);
					self.progress.store(false, Ordering::Release);
					self.broker.send_replace(Some(broker.clone()));
					state = "recovering";
					self.mode(state);
					tracing::info!(
						worker_pid = std::process::id(),
						consumes = self.worker,
						"activation transport ready"
					);
					let mut observation = Instant::now();
					let mut publish_after = Instant::now();
					let mut publish_backoff = Duration::from_millis(500);
					let mut ticks = tokio::time::interval(Duration::from_millis(250));
					ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
					loop {
						tokio::select! {
							biased;
							_ = stopped(&mut stopping) => break,
							_ = ticks.tick() => {}
						}
						if self.failed.load(Ordering::Acquire)
							|| broker.disconnected()
							|| !broker.connected()
						{
							break;
						}
						if Instant::now() < publish_after {
							continue;
						}
						match activation::publish(self.repository.as_ref(), broker.as_ref()).await {
							Ok(Publication::Backpressured) => {
								// Consumers must keep draining retained messages to free capacity.
								metrics::counter!("aidash_activation_publish_errors_total")
									.increment(1);
								publish_after = Instant::now() + publish_backoff;
								publish_backoff = (publish_backoff * 2).min(Duration::from_secs(5));
							}
							Ok(Publication::Published(published)) => {
								publish_backoff = Duration::from_millis(500);
								if !self.worker && published > 0 {
									self.progress.store(true, Ordering::Release);
								}
								if observation.elapsed() >= Duration::from_secs(1) {
									let _ = self.repository.observe().await;
									observation = Instant::now();
								}
								// Subscription readiness is distinct from observed delivery.
								if self.progress.load(Ordering::Acquire) && state != "event_driven"
								{
									state = "event_driven";
									self.mode(state);
								}
							}
							Err(aidash_application::Error::TransactionPending) => {}
							Err(error) => {
								tracing::warn!(%error, "activation publication failed; durable obligations retained");
								metrics::counter!("aidash_activation_publish_errors_total")
									.increment(1);
								break;
							}
						}
					}
				}
				Err(error) => {
					// The error text comes from bounded local categories; never log URL/credentials.
					tracing::warn!(%error,"activation setup failed; database recovery remains available");
				}
			}
			self.broker.send_replace(None);
			if *stopping.borrow() {
				break;
			}
			self.mode("fallback");
			// Jitter is included in the five-second cap.
			let jitter = u64::from(uuid::Uuid::new_v4().as_bytes()[0]) % 101;
			let delay = (backoff + Duration::from_millis(jitter)).min(Duration::from_secs(5));
			tokio::select! { _ = tokio::time::sleep(delay) => {}, _ = stopped(&mut stopping) => break }
			backoff = (backoff * 2).min(Duration::from_millis(4900));
		}
		self.broker.send_replace(None);
		self.mode("stopping");
		Ok(())
	}
	fn mode(&self, mode: &'static str) {
		for value in ["event_driven", "recovering", "fallback", "stopping"] {
			metrics::gauge!("aidash_activation_mode", "mode" => value).set(if value == mode {
				1.0
			} else {
				0.0
			});
		}
		tracing::info!(
			mode,
			worker_pid = std::process::id(),
			"activation transport state"
		);
	}
	/// Each invocation is exactly one slot. There is no speculative prefetch:
	/// the finite one-message batch expires before this slot can do recovery.
	pub async fn worker(
		self: Arc<Self>,
		repository: Arc<dyn ActivationRepository>,
		mut stopping: watch::Receiver<bool>,
	) -> Result<()> {
		let mut broker_rx = self.broker.subscribe();
		let mut last_scan = Instant::now() - self.settings.recovery.max(self.settings.fallback);
		let mut startup = true;
		while !*stopping.borrow() {
			let broker = broker_rx.borrow_and_update().clone();
			let cadence = if broker.as_ref().is_some_and(|b| !b.disconnected())
				&& !self.failed.load(Ordering::Acquire)
			{
				self.settings.recovery
			} else {
				self.settings.fallback
			};
			if last_scan.elapsed() >= cadence {
				last_scan = Instant::now();
				let result = async {
					let visibility = repository.visibility().await?;
					metrics::counter!("aidash_activation_recovery_scans_total").increment(1);
					let run = repository.recover().await?;
					metrics::gauge!("aidash_activation_last_recovery_timestamp_seconds")
						.set(chrono::Utc::now().timestamp() as f64);
					if let Some((run, token)) = run {
						visibility.advance(run, token).await?;
					}
					Result::Ok(())
				}
				.await;
				if let Err(error) = result {
					tracing::warn!(%error,"activation recovery deferred");
				} else if startup {
					startup = false;
					tracing::info!(
						worker_pid = std::process::id(),
						"activation startup recovery complete"
					);
				}
				if *stopping.borrow() {
					break;
				}
			}
			let Some(broker) = broker else {
				tokio::select! { _ = tokio::time::sleep(Duration::from_millis(100)) => {}, _ = stopped(&mut stopping) => break }
				continue;
			};
			if self
				.settings
				.test_pause_file
				.as_ref()
				.is_some_and(|path| path.exists())
			{
				tokio::time::sleep(Duration::from_millis(100)).await;
				continue;
			}
			// Fetch uses no_wait: no server-side outstanding long pull can deliver
			// after this slot has switched to a recovery claim.
			let pull_budget = cadence
				.saturating_sub(last_scan.elapsed())
				.min(Duration::from_millis(400));
			let delivery = tokio::time::timeout(pull_budget, broker.fetch())
				.await
				.unwrap_or_else(|_| {
					if last_scan.elapsed() >= cadence {
						Ok(None)
					} else {
						Err(activation::unavailable())
					}
				});
			let result = match delivery {
				Ok(Some(message)) if !*stopping.borrow() && !broker.disconnected() => {
					// A test barrier can arrive while a pull is already in flight.
					// Retain its delivery without claiming or acknowledging until resumed.
					while self
						.settings
						.test_pause_file
						.as_ref()
						.is_some_and(|p| p.exists())
					{
						tokio::select! {
							_ = tokio::time::sleep(Duration::from_millis(25)) => {},
							_ = stopped(&mut stopping) => return Ok(()),
						}
					}
					if *stopping.borrow() || broker.disconnected() {
						continue;
					}
					self.receive(repository.as_ref(), message.as_ref()).await
				}
				Ok(_) => {
					tokio::time::sleep(Duration::from_millis(25)).await;
					Ok(())
				}
				Err(error) => Err(error),
			};
			if let Err(error) = result {
				if matches!(error, aidash_application::Error::External(_)) {
					self.failed.store(true, Ordering::Release);
				}
				tracing::warn!(%error,"activation delivery deferred");
				tokio::time::sleep(Duration::from_millis(100)).await;
			}
		}
		Ok(())
	}
	async fn receive(
		&self,
		repository: &dyn ActivationRepository,
		message: &dyn ActivationDelivery,
	) -> Result<()> {
		let disposition = activation::receive(repository, message, || {
			self.progress.store(true, Ordering::Release)
		})
		.await?;
		if disposition.ack_failed {
			metrics::counter!("aidash_activation_ack_errors_total").increment(1);
		}
		if let Some((run, token, visibility)) = disposition.claimed {
			// The test-only crash checkpoint still precedes effects and heartbeat.
			while self
				.settings
				.test_after_ack_pause_file
				.as_ref()
				.is_some_and(|p| p.exists())
			{
				tokio::time::sleep(Duration::from_millis(25)).await;
			}
			visibility.advance(*run, token).await?;
		}
		Ok(())
	}
}
async fn stopped(receiver: &mut watch::Receiver<bool>) {
	while !*receiver.borrow_and_update() {
		if receiver.changed().await.is_err() {
			return;
		}
	}
}
#[cfg(test)]
mod tests;
