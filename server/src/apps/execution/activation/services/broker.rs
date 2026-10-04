//! Compatibility surface over the injected external transport implementation.
use super::Settings;
use crate::Result;
#[derive(Clone)]
pub struct Broker {
	pub context: async_nats::jetstream::Context,
	pub subject: String,
	pub stream_name: String,
	pub consumer: Option<async_nats::jetstream::consumer::PullConsumer>,
}
impl From<aidash_integrations::activation::Broker> for Broker {
	fn from(broker: aidash_integrations::activation::Broker) -> Self {
		Self {
			context: broker.context,
			subject: broker.subject,
			stream_name: broker.stream_name,
			consumer: broker.consumer,
		}
	}
}
impl Broker {
	pub fn names(node: &str, settings: &Settings) -> (String, String) {
		aidash_integrations::activation::Broker::names(
			node,
			&crate::bootstrap::activation_broker_configuration(settings),
		)
	}
	pub async fn connect(url: &str, node: &str, settings: &Settings, worker: bool) -> Result<Self> {
		aidash_integrations::activation::Broker::connect(
			url,
			node,
			&crate::bootstrap::activation_broker_configuration(settings),
			worker,
		)
		.await
		.map(Into::into)
		.map_err(Into::into)
	}
	pub async fn provision(url: &str, node: &str, settings: &Settings) -> Result<Self> {
		aidash_integrations::activation::Broker::provision(
			url,
			node,
			&crate::bootstrap::activation_broker_configuration(settings),
		)
		.await
		.map(Into::into)
		.map_err(Into::into)
	}
}
