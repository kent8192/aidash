use super::{
	Broker, Settings,
	broker::Publication,
	durable::{self, Envelope, Handoff},
};
use crate::{Result, federation::Federation, harness::Harness, transactions::gate::ReadLease};
use async_nats::jetstream::AckKind;
use futures_util::StreamExt;
use std::{
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
	time::Duration,
};
use tokio::{sync::watch, time::Instant};

pub struct Runtime {
	f: Federation,
	settings: Settings,
	worker: bool,
	broker: watch::Sender<Option<Arc<Broker>>>,
	failed: AtomicBool,
	progress: AtomicBool,
}
impl Runtime {
	pub fn new(mut f: Federation, settings: Settings, worker: bool) -> Arc<Self> {
		if let Ok(url) = std::env::var("AIDASH_ACTIVATION_NATS_URL") {
			f.config.nats_url = url;
		}
		let (broker, _) = watch::channel(None);
		Arc::new(Self {
			f,
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
					let visibility = ReadLease::begin(&self.f.store).await?;
					let reconciled = durable::reconcile(&self.f.store).await?;
					drop(visibility);
					if reconciled < durable::RECONCILE_BATCH_SIZE {
						break;
					}
				}
				Broker::connect(
					&self.f.config.nats_url,
					&self.f.config.node_id,
					&self.settings,
					self.worker,
				)
				.await
			};
			let result = tokio::select! {
				result = setup => result,
				_ = crate::lifecycle::stopped(&mut stopping) => break,
			};
			match result {
				Ok(broker) => {
					backoff = Duration::from_millis(500);
					self.failed.store(false, Ordering::Release);
					self.progress.store(false, Ordering::Release);
					let broker = Arc::new(broker);
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
							_ = crate::lifecycle::stopped(&mut stopping) => break,
							_ = ticks.tick() => {}
						}
						if self.failed.load(Ordering::Acquire)
							|| broker.disconnected.load(Ordering::Acquire)
							|| broker.context.client().connection_state()
								!= async_nats::connection::State::Connected
						{
							break;
						}
						if Instant::now() < publish_after {
							continue;
						}
						match broker.publish(&self.f.store).await {
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
									let _ = durable::observe(&self.f.store).await;
									observation = Instant::now();
								}
								// Subscription readiness is distinct from observed delivery.
								if self.progress.load(Ordering::Acquire) && state != "event_driven"
								{
									state = "event_driven";
									self.mode(state);
								}
							}
							Err(crate::Error::TransactionPending) => {}
							Err(_) => {
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
			tokio::select! { _ = tokio::time::sleep(delay) => {}, _ = crate::lifecycle::stopped(&mut stopping) => break }
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
		h: Harness,
		mut stopping: watch::Receiver<bool>,
	) -> Result<()> {
		let mut broker_rx = self.broker.subscribe();
		let mut last_scan = Instant::now() - self.settings.recovery.max(self.settings.fallback);
		let mut startup = true;
		while !*stopping.borrow() {
			let broker = broker_rx.borrow_and_update().clone();
			let cadence = if broker
				.as_ref()
				.is_some_and(|b| !b.disconnected.load(Ordering::Acquire))
				&& !self.failed.load(Ordering::Acquire)
			{
				self.settings.recovery
			} else {
				self.settings.fallback
			};
			if last_scan.elapsed() >= cadence {
				last_scan = Instant::now();
				let result = async {
					let visibility = ReadLease::begin(&h.federation.store).await?;
					metrics::counter!("aidash_activation_recovery_scans_total").increment(1);
					let run =
						durable::recover(&h.federation.store, h.federation.config.lease_seconds)
							.await?;
					metrics::gauge!("aidash_activation_last_recovery_timestamp_seconds")
						.set(chrono::Utc::now().timestamp() as f64);
					if let Some((run, token)) = run {
						h.advance_leased(run, token, visibility).await?;
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
				tokio::select! { _ = tokio::time::sleep(Duration::from_millis(100)) => {}, _ = crate::lifecycle::stopped(&mut stopping) => break }
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
			let Some(consumer) = &broker.consumer else {
				return Err(crate::Error::Invalid(
					"worker has no activation consumer".into(),
				));
			};
			// Fetch uses no_wait: no server-side outstanding long pull can deliver
			// after this slot has switched to a recovery claim.
			let pull_budget = cadence
				.saturating_sub(last_scan.elapsed())
				.min(Duration::from_millis(400));
			let delivery = tokio::time::timeout(pull_budget, async {
				let mut batch = consumer
					.fetch()
					.max_messages(1)
					.expires(Duration::from_millis(250))
					.messages()
					.await
					.map_err(|_| super::broker::unavailable())?;
				batch
					.next()
					.await
					.transpose()
					.map_err(|_| super::broker::unavailable())
			})
			.await
			.unwrap_or_else(|_| {
				if last_scan.elapsed() >= cadence {
					Ok(None)
				} else {
					Err(super::broker::unavailable())
				}
			});
			let result = match delivery {
				Ok(Some(message))
					if !*stopping.borrow() && !broker.disconnected.load(Ordering::Acquire) =>
				{
					self.receive(&h, &message).await
				}
				Ok(_) => {
					tokio::time::sleep(Duration::from_millis(25)).await;
					Ok(())
				}
				Err(error) => Err(error),
			};
			if let Err(error) = result {
				if matches!(error, crate::Error::External(_)) {
					self.failed.store(true, Ordering::Release);
				}
				tracing::warn!(%error,"activation delivery deferred");
				tokio::time::sleep(Duration::from_millis(100)).await;
			}
		}
		Ok(())
	}
	async fn receive(&self, h: &Harness, message: &async_nats::jetstream::Message) -> Result<()> {
		let envelope = serde_json::from_slice::<Envelope>(&message.payload);
		let envelope = match envelope {
			Ok(value)
				if value.version == 1
					&& value.node_id == h.federation.store.node_id
					&& value.generation > 0 =>
			{
				value
			}
			other => {
				let reason = match other {
					Ok(v) if v.version != 1 => "unsupported_version",
					Ok(_) => "wrong_scope",
					Err(_) => "malformed",
				};
				return self.quarantine(h, message, reason).await;
			}
		};
		let disposition = async {
			let visibility = ReadLease::begin(&h.federation.store).await?;
			let handoff = durable::claim(
				&h.federation.store,
				&envelope,
				h.federation.config.lease_seconds,
			)
			.await?;
			Result::Ok((visibility, handoff))
		}
		.await;
		let (visibility, handoff) = match disposition {
			Ok(value) => value,
			Err(error) => {
				// This is a delayed negative acknowledgement, never evidence of
				// committed responsibility. Unknown commit results remain fenced.
				let _ = tokio::time::timeout(
					Duration::from_secs(1),
					message.ack_with(AckKind::Nak(Some(Duration::from_millis(250)))),
				)
				.await;
				return Err(error);
			}
		};
		if matches!(handoff, Handoff::Invalid) {
			return self.quarantine(h, message, "invalid_reference").await;
		}
		self.progress.store(true, Ordering::Release);
		// An ACK error cannot undo a committed lease; execution still uses that
		// lease and redelivery sees the same durable disposition.
		if !tokio::time::timeout(Duration::from_secs(1), message.ack())
			.await
			.is_ok_and(|r| r.is_ok())
		{
			metrics::counter!("aidash_activation_ack_errors_total").increment(1);
		}
		if let Handoff::Claimed(run, token) = handoff {
			// Explicit test-only crash checkpoint after the durable handoff/ACK,
			// before any effect and before the execution heartbeat starts.
			while self
				.settings
				.test_after_ack_pause_file
				.as_ref()
				.is_some_and(|p| p.exists())
			{
				tokio::time::sleep(Duration::from_millis(25)).await;
			}
			h.advance_leased(*run, token, visibility).await?;
		}
		Ok(())
	}
	async fn quarantine(
		&self,
		h: &Harness,
		message: &async_nats::jetstream::Message,
		reason: &'static str,
	) -> Result<()> {
		durable::quarantine(
			&h.federation.store,
			&message.payload,
			reason,
			message.info().ok().map(|i| i.stream_sequence),
		)
		.await?;
		message
			.ack_with(AckKind::Term)
			.await
			.map_err(|_| super::broker::unavailable())?;
		Ok(())
	}
}
