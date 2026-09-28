use super::{Settings, durable};
use crate::{Error, Result, store::Store};
use async_nats::jetstream::{
	self,
	consumer::{AckPolicy, PullConsumer, pull},
	stream,
};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::{
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
	time::Duration,
};
use uuid::Uuid;

#[derive(Clone)]
pub struct Broker {
	pub context: jetstream::Context,
	pub subject: String,
	pub stream_name: String,
	pub consumer: Option<PullConsumer>,
	pub(super) disconnected: Arc<AtomicBool>,
}
impl Broker {
	pub fn names(node: &str, settings: &Settings) -> (String, String) {
		// Length-delimited, collision-resistant, stable across Pod replacement.
		let key = format!(
			"{}:{}:{}:{}:v1",
			settings.namespace.len(),
			settings.namespace,
			node.len(),
			node
		);
		let digest = format!("{:x}", Sha256::digest(key.as_bytes()));
		(
			format!("AIDASH_ACT_V1_{digest}"),
			format!("aidash.activation.v1.{digest}"),
		)
	}
	pub async fn connect(url: &str, node: &str, settings: &Settings, worker: bool) -> Result<Self> {
		tokio::time::timeout(
			Duration::from_secs(5),
			Self::setup(url, node, settings, worker),
		)
		.await
		.map_err(|_| Error::External("activation setup timeout".into()))?
	}
	async fn setup(url: &str, node: &str, settings: &Settings, worker: bool) -> Result<Self> {
		let mut address = reqwest::Url::parse(url)
			.map_err(|_| Error::Invalid("invalid activation broker address".into()))?;
		let disconnected = Arc::new(AtomicBool::new(false));
		let lost = disconnected.clone();
		let mut options = async_nats::ConnectOptions::new().event_callback(move |event| {
			let lost = lost.clone();
			async move {
				if matches!(
					event,
					async_nats::Event::Disconnected | async_nats::Event::Closed
				) {
					lost.store(true, Ordering::Release);
					metrics::counter!("aidash_activation_disconnects_total").increment(1);
				}
			}
		});
		let decode = |value: &str| {
			percent_encoding::percent_decode_str(value)
				.decode_utf8()
				.map(|s| s.into_owned())
				.map_err(|_| {
					Error::Invalid("invalid activation broker authentication encoding".into())
				})
		};
		if !address.username().is_empty() {
			options = match address.password() {
				Some(password) => {
					options.user_and_password(decode(address.username())?, decode(password)?)
				}
				None => options.token(decode(address.username())?),
			};
			address
				.set_username("")
				.map_err(|_| Error::Invalid("invalid activation broker address".into()))?;
			address
				.set_password(None)
				.map_err(|_| Error::Invalid("invalid activation broker address".into()))?;
		}
		if let Some(path) = std::env::var_os("AIDASH_ACTIVATION_NATS_CREDENTIALS") {
			options = options
				.credentials_file(path)
				.await
				.map_err(|_| unavailable())?;
		}
		let client = options
			.connect(address.as_str())
			.await
			.map_err(|_| unavailable())?;
		let context = jetstream::new(client);
		let (stream_name, subject) = Self::names(node, settings);
		let wanted = stream::Config {
			name: stream_name.clone(),
			subjects: vec![subject.clone()],
			retention: stream::RetentionPolicy::WorkQueue,
			storage: stream::StorageType::File,
			discard: stream::DiscardPolicy::New,
			max_age: settings.max_age,
			max_bytes: settings.max_bytes,
			duplicate_window: Duration::from_secs(120).min(settings.max_age),
			num_replicas: settings.replicas,
			..Default::default()
		};
		let stream = if settings.bootstrap {
			context
				.get_or_create_stream(wanted.clone())
				.await
				.map_err(|_| unavailable())?
		} else {
			context
				.get_stream(&stream_name)
				.await
				.map_err(|_| unavailable())?
		};
		let actual = &stream.cached_info().config;
		let maximum_message_size = maximum_message_size(node)?;
		if actual.subjects != wanted.subjects
			|| actual.retention != wanted.retention
			|| actual.storage != wanted.storage
			|| actual.discard != wanted.discard
			|| actual.max_age != wanted.max_age
			|| actual.max_bytes != wanted.max_bytes
			|| actual.duplicate_window != wanted.duplicate_window
			|| actual.num_replicas != wanted.num_replicas
			|| (actual.max_message_size > 0
				&& (actual.max_message_size as usize) < maximum_message_size)
			|| actual.max_messages > 0
			|| actual.max_messages_per_subject > 0
			|| actual.sealed
			|| actual.allow_rollup
		{
			return Err(Error::Invalid(
				"activation stream configuration mismatch; operator repair required".into(),
			));
		}
		let config = pull::Config {
			durable_name: Some("workers-v1".into()),
			ack_policy: AckPolicy::Explicit,
			ack_wait: Duration::from_secs(30),
			filter_subject: subject.clone(),
			max_batch: 1,
			max_ack_pending: 1024,
			max_waiting: 1024,
			max_expires: Duration::from_secs(1),
			..Default::default()
		};
		let consumer = if worker || settings.bootstrap {
			let consumer: PullConsumer = if settings.bootstrap {
				stream
					.get_or_create_consumer("workers-v1", config.clone())
					.await
					.map_err(|_| unavailable())?
			} else {
				stream
					.get_consumer("workers-v1")
					.await
					.map_err(|_| unavailable())?
			};
			let actual = &consumer.cached_info().config;
			if actual.ack_policy != config.ack_policy
				|| actual.ack_wait != config.ack_wait
				|| actual.filter_subject != config.filter_subject
				|| actual.max_batch != 1
				|| actual.max_ack_pending != 1024
				|| actual.max_waiting != 1024
				|| actual.max_expires != config.max_expires
				|| actual.deliver_subject.is_some()
				|| actual.deliver_policy != config.deliver_policy
				|| actual.max_deliver > 0
				|| !actual.backoff.is_empty()
				|| actual.inactive_threshold != Duration::ZERO
				|| actual.memory_storage
			{
				return Err(Error::Invalid(
					"activation consumer configuration mismatch; operator repair required".into(),
				));
			}
			worker.then_some(consumer)
		} else {
			None
		};
		Ok(Self {
			context,
			subject,
			stream_name,
			consumer,
			disconnected,
		})
	}
	pub async fn provision(url: &str, node: &str, settings: &Settings) -> Result<Self> {
		let mut settings = settings.clone();
		settings.bootstrap = true;
		Self::connect(url, node, &settings, true).await
	}
	pub(super) async fn publish(&self, store: &Store) -> Result<usize> {
		if self.disconnected.load(Ordering::Acquire) {
			return Err(unavailable());
		}
		let mut visibility = crate::transactions::gate::ReadLease::begin(store).await?;
		let token = Uuid::new_v4();
		let rows = durable::publish_batch(store, token).await?;
		// Broker acknowledgements cannot retain the ordinary-state visibility lock.
		visibility.suspend().await?;
		let count = rows.len();
		let mut sends = futures_util::stream::iter(rows.into_iter().map(|row| async move {
			let mut headers = async_nats::HeaderMap::new();
			headers.insert(
				"Nats-Msg-Id",
				format!("{}:{}", row.id, row.publication_epoch),
			);
			let bytes = serde_json::to_vec(&row.envelope(&store.node_id))?;
			tokio::time::timeout(Duration::from_secs(2), async {
				self.context
					.publish_with_headers(self.subject.clone(), headers, bytes.into())
					.await
					.map_err(|_| unavailable())?
					.await
					.map_err(|_| unavailable())?;
				Result::Ok(())
			})
			.await
			.map_err(|_| unavailable())??;
			durable::published(store, &row, token).await?;
			metrics::counter!("aidash_activation_published_total").increment(1);
			Result::Ok(())
		}))
		.buffer_unordered(8);
		while let Some(result) = sends.next().await {
			result?;
		}
		Ok(count)
	}
}
pub(super) fn unavailable() -> Error {
	Error::External("activation broker unavailable".into())
}

/// JetStream's message limit includes the payload and serialized NATS headers.
fn maximum_message_size(node: &str) -> Result<usize> {
	let envelope = durable::Envelope {
		version: 1,
		node_id: node.into(),
		run_id: Uuid::nil(),
		activation_id: Uuid::nil(),
		generation: i64::MAX,
	};
	let header = format!(
		"NATS/1.0\r\nNats-Msg-Id: {}:{}\r\n\r\n",
		Uuid::nil(),
		i64::MAX
	);
	Ok(serde_json::to_vec(&envelope)?.len() + header.len())
}
