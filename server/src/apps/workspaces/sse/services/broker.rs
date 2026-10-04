use super::*;
use futures_util::StreamExt;

impl Service {
	pub fn shutdown(&self) {
		self.inner.stopping.send_replace(true);
	}

	/// Independent ordinary subscriptions fan out to every HTTP replica.
	pub async fn run(
		&self,
		url: &str,
		node: &str,
		mut stopping: watch::Receiver<bool>,
	) -> Result<()> {
		crate::config::validate_node_id(node)?;
		metrics::gauge!("aidash_sse_subscriber_ready").set(0.0);
		let subject = format!(
			"aidash.{}.events",
			node.strip_prefix("aidash://").expect("validated Node")
		);
		let mut delay = Duration::from_millis(250);
		let mut local_stopping = self.inner.stopping.subscribe();
		loop {
			if *stopping.borrow() || *local_stopping.borrow() {
				break;
			}
			let (fault, mut failed) = watch::channel(false);
			let connect = async {
				let options = async_nats::ConnectOptions::new()
					.connection_timeout(Duration::from_secs(5))
					.subscription_capacity(256)
					// Recreate and flush the subscription after every disconnect;
					// a client Connected callback alone cannot prove readiness.
					.max_reconnects(0)
					.event_callback(move |event| {
						let fault = fault.clone();
						async move {
							if matches!(event, async_nats::Event::Disconnected | async_nats::Event::Closed | async_nats::Event::SlowConsumer(_) | async_nats::Event::ServerError(_) | async_nats::Event::ClientError(_)) {
								fault.send_replace(true);
							}
						}
					});
				let (address, options) =
					crate::bus::connection_options(url, options).map_err(|_| ())?;
				let client = options.connect(address).await.map_err(|_| ())?;
				let messages = client.subscribe(subject.clone()).await.map_err(|_| ())?;
				client.flush().await.map_err(|_| ())?;
				Ok::<_, ()>((client, messages))
			};
			let connected = tokio::select! {
				biased;
				_ = crate::lifecycle::stopped(&mut stopping) => break,
				_ = crate::lifecycle::stopped(&mut local_stopping) => break,
				result = tokio::time::timeout(Duration::from_secs(5), connect) => result,
			};
			if let Ok(Ok((_client, mut messages))) = connected
				&& !*failed.borrow()
			{
				self.inner.counts.ready.store(true, Ordering::Relaxed);
				metrics::gauge!("aidash_sse_subscriber_ready").set(1.0);
				metrics::gauge!("aidash_sse_last_subscription_timestamp_seconds").set(timestamp());
				self.notify(None, true);
				tracing::info!(
					"SSE notification subscription ready; reconciling connected streams"
				);
				delay = Duration::from_millis(250);
				loop {
					if *failed.borrow() || *stopping.borrow() || *local_stopping.borrow() {
						break;
					}
					tokio::select! {
						biased;
						_ = crate::lifecycle::stopped(&mut stopping) => break,
						_ = crate::lifecycle::stopped(&mut local_stopping) => break,
						_ = failed.changed() => break,
						message = messages.next() => {
							let Some(message) = message else { break; };
							self.hint(&message.payload, node);
						}
					}
				}
			}
			if self.inner.counts.ready.swap(false, Ordering::Relaxed) {
				self.notify(None, true);
			}
			metrics::gauge!("aidash_sse_subscriber_ready").set(0.0);
			if *stopping.borrow() || *local_stopping.borrow() {
				break;
			}
			metrics::counter!("aidash_sse_subscriber_retries_total").increment(1);
			tracing::warn!(
				"SSE notification transport unavailable; canonical reconciliation remains active"
			);
			let jitter = u64::from(Uuid::new_v4().as_bytes()[0]);
			let millis = 250 + jitter * (delay.as_millis() as u64 - 250) / 255;
			tokio::select! {
				_ = crate::lifecycle::stopped(&mut stopping) => break,
				_ = crate::lifecycle::stopped(&mut local_stopping) => break,
				_ = tokio::time::sleep(Duration::from_millis(millis)) => {},
			}
			delay = (delay * 2).min(Duration::from_secs(5));
		}
		self.inner.counts.ready.store(false, Ordering::Relaxed);
		metrics::gauge!("aidash_sse_subscriber_ready").set(0.0);
		self.shutdown();
		Ok(())
	}

	fn hint(&self, payload: &[u8], node: &str) {
		// Unknown fields, including a potentially large data body, are skipped.
		// Neither sequence nor dataref is used to move a cursor or fetch data.
		match serde_json::from_slice::<Hint>(payload) {
			Ok(hint) if hint.specversion == "1.0" && hint.source == node && !hint.id.is_nil() => {
				self.inner
					.counts
					.notifications
					.fetch_add(1, Ordering::Relaxed);
				metrics::counter!("aidash_sse_notifications_total").increment(1);
				self.notify(hint.subject, false);
			}
			_ => {
				self.inner.counts.rejected.fetch_add(1, Ordering::Relaxed);
				metrics::counter!("aidash_sse_rejected_notifications_total").increment(1);
			}
		}
	}
}

pub(crate) use crate::apps::workspaces::sse::serializers::broker::Hint;

#[cfg(test)]
#[path = "../tests/services_broker_tests.rs"]
mod tests;
