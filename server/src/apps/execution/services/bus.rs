//! Event service adapter; publication and admission are application use cases.
use crate::{Error, Result, federation::Federation};
use aidash_integrations::nats::NatsBus;
use std::time::Duration;
#[derive(Clone)]
pub struct EventBus {
	transport: NatsBus,
}
impl std::ops::Deref for EventBus {
	type Target = NatsBus;
	fn deref(&self) -> &Self::Target {
		&self.transport
	}
}
impl EventBus {
	/// Broker connection and consumer recovery never gate HTTP or worker startup.
	pub async fn run(f: Federation) -> Result<()> {
		loop {
			let result = async {
				let bus = tokio::time::timeout(
					Duration::from_secs(15),
					Self::connect(&f.config.nats_url, &f.config.node_id),
				)
				.await
				.map_err(|_| Error::External("event bus connection timed out".into()))??;
				tokio::select! {
					result = bus.publisher(f.clone()) => result,
					result = bus.consumer(f.clone()) => result,
				}
			}
			.await;
			if let Err(error) = result {
				tracing::warn!(%error, "event bus unavailable; durable state retained for retry");
			}
			tokio::time::sleep(Duration::from_secs(2)).await;
		}
	}
	pub async fn connect(url: &str, node_id: &str) -> Result<Self> {
		Ok(Self {
			transport: crate::bootstrap::nats_transport(url, node_id).await?,
		})
	}
	pub async fn publish_once(&self, f: &Federation) -> Result<usize> {
		aidash_application::events::publish(
			&crate::bootstrap::event_outbox(&f.store),
			&self.transport,
		)
		.await
		.map_err(Into::into)
	}
	pub async fn publisher(&self, f: Federation) -> Result<()> {
		let mut active_until = tokio::time::Instant::now();
		loop {
			let published = self.publish_once(&f).await;
			let delay = match published {
				Ok(count) => {
					if count > 0 {
						active_until = tokio::time::Instant::now() + Duration::from_secs(1);
					}
					// Spread sustained notifications across smaller batches instead
					// of bursting every 250 ms into serialized per-frame audits.
					// Quiescent publishers retain their existing idle scan cadence.
					if tokio::time::Instant::now() < active_until {
						Duration::from_millis(50)
					} else {
						Duration::from_millis(250)
					}
				}
				Err(e) => {
					tracing::warn!(error=%e,"outbox publish failed; retained for retry");
					Duration::from_millis(250)
				}
			};
			tokio::time::sleep(delay).await;
		}
	}
	pub async fn consumer(&self, f: Federation) -> Result<()> {
		let inbox = crate::bootstrap::event_inbox(&f.store)?;
		let mut subscription = self.transport.subscribe().await?;
		loop {
			aidash_application::events::receive(&inbox, &mut subscription, &f).await?;
		}
	}
}
#[path = "bus/connection.rs"]
mod connection;
pub(crate) use connection::options as connection_options;
