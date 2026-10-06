//! JetStream transport, wire envelopes, payload limits, and acknowledgements.
use crate::{Error, Result};
use aidash_application::ports::events::*;
use aidash_domain::Event;
use async_nats::jetstream::{
	self,
	consumer::{AckPolicy, pull},
	stream,
};
use async_trait::async_trait;
use futures_util::StreamExt;
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone)]
pub struct NatsBus {
	pub context: jetstream::Context,
	pub stream_name: String,
	pub subject: String,
}
impl NatsBus {
	pub async fn connect(url: &str, node_id: &str) -> Result<Self> {
		let (address, options) = connection::options(url, async_nats::ConnectOptions::new())?;
		let client = options
			.connect(address)
			.await
			.map_err(|e| Error::External(e.to_string()))?;
		let context = jetstream::new(client);
		let suffix = node_id
			.strip_prefix("aidash://")
			.ok_or_else(|| Error::Invalid("invalid node id".into()))?;
		let stream_name = format!("AIDASH_{}", suffix.replace('-', "_"));
		let subject = format!("aidash.{suffix}.events");
		context
			.get_or_create_stream(stream::Config {
				name: stream_name.clone(),
				subjects: vec![subject.clone()],
				storage: stream::StorageType::File,
				duplicate_window: Duration::from_secs(120),
				..Default::default()
			})
			.await
			.map_err(|e| Error::External(e.to_string()))?;
		Ok(Self {
			context,
			stream_name,
			subject,
		})
	}
	pub async fn subscribe(&self) -> Result<NatsSubscription> {
		let stream = self
			.context
			.get_stream(&self.stream_name)
			.await
			.map_err(|error| Error::External(error.to_string()))?;
		let consumer = stream
			.get_or_create_consumer(
				"execution",
				pull::Config {
					durable_name: Some("execution".into()),
					ack_policy: AckPolicy::Explicit,
					filter_subject: self.subject.clone(),
					ack_wait: Duration::from_secs(30),
					..Default::default()
				},
			)
			.await
			.map_err(|error| Error::External(error.to_string()))?;
		Ok(NatsSubscription {
			consumer,
			messages: None,
		})
	}
}
#[async_trait]
impl EventPublisher for NatsBus {
	async fn publish(&self, event: &Event) -> Result<()> {
		let mut headers = async_nats::HeaderMap::new();
		let message_id = event.id.to_string();
		let header_len = b"NATS/1.0\r\nNats-Msg-Id: \r\n\r\n".len() + message_id.len();
		headers.insert("Nats-Msg-Id", message_id);
		let mut envelope = event.cloud_event();
		let mut payload = envelope.to_string();
		let limit = self.context.client().server_info().max_payload;
		// Keep the durable payload in PostgreSQL and publish a reference
		// when broker limits cannot accommodate the complete event.
		if payload.len().saturating_add(header_len) > limit {
			envelope
				.as_object_mut()
				.expect("CloudEvent object")
				.remove("data");
			envelope["dataref"] =
				serde_json::json!(format!("/api/events?after={}", event.sequence - 1));
			payload = envelope.to_string();
		}
		// Even reference envelopes must fit before HPUB: oversize frames
		// disconnect the shared connection and disrupt healthy events.
		if payload.len().saturating_add(header_len) > limit {
			return Err(Error::External(format!(
				"event exceeds NATS max_payload {limit}"
			)));
		}
		self.context
			.publish_with_headers(self.subject.clone(), headers, payload.into())
			.await
			.map_err(|error| Error::External(error.to_string()))?
			.await
			.map_err(|error| Error::External(error.to_string()))?;
		Ok(())
	}
}
pub struct NatsSubscription {
	consumer: jetstream::consumer::Consumer<pull::Config>,
	messages: Option<pull::Stream>,
}
#[async_trait]
impl EventSubscription for NatsSubscription {
	async fn next(&mut self) -> Result<Box<dyn EventDelivery>> {
		loop {
			if self.messages.is_none() {
				self.messages = Some(
					self.consumer
						.messages()
						.await
						.map_err(|error| Error::External(error.to_string()))?,
				);
			}
			if let Some(message) = self
				.messages
				.as_mut()
				.expect("initialized stream")
				.next()
				.await
			{
				return Ok(Box::new(NatsDelivery(
					message.map_err(|error| Error::External(error.to_string()))?,
				)));
			}
			self.messages = None;
		}
	}
}
struct NatsDelivery(jetstream::Message);
#[async_trait]
impl EventDelivery for NatsDelivery {
	fn event_id(&self) -> Option<Uuid> {
		serde_json::from_slice::<serde_json::Value>(&self.0.payload)
			.ok()
			.and_then(|event| event["id"].as_str().and_then(|id| id.parse().ok()))
	}
	async fn acknowledge(self: Box<Self>) -> Result<()> {
		self.0
			.ack()
			.await
			.map_err(|error| Error::External(error.to_string()))
	}
	async fn discard(self: Box<Self>) -> Result<()> {
		self.0
			.ack_with(jetstream::AckKind::Term)
			.await
			.map_err(|error| Error::External(error.to_string()))
	}
}
pub mod connection;
