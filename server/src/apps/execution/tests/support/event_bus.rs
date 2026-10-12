use crate::endpoint::{EndpointFixture, endpoint};
use aidash_server::{Result, bus::EventBus};
use reinhardt::test::fixtures::nats_container;
use reinhardt::test::testcontainers::{ContainerAsync, GenericImage};
use rstest::fixture;
use std::future::Future;
use tokio::task::JoinHandle;

pub struct BusFixture {
	pub app: EndpointFixture,
	pub bus: EventBus,
	_broker: ContainerAsync<GenericImage>,
}

#[fixture]
pub fn event_bus(
	#[future] endpoint: EndpointFixture,
	#[future] nats_container: (ContainerAsync<GenericImage>, u16, String),
) -> impl Future<Output = BusFixture> {
	let endpoint = Box::pin(endpoint);
	let nats_container = Box::pin(nats_container);
	async move {
		let (broker, _port, url) = nats_container.await;
		let app = endpoint.await;
		let bus = EventBus::connect(&url, &app.runtime.config.node_id)
			.await
			.unwrap();
		BusFixture {
			app,
			bus,
			_broker: broker,
		}
	}
}

pub struct ConsumerTask(JoinHandle<Result<()>>);

impl BusFixture {
	pub fn consumer(&self) -> ConsumerTask {
		let bus = self.bus.clone();
		let runtime = self.app.runtime.clone();
		ConsumerTask(tokio::spawn(async move { bus.consumer(runtime).await }))
	}
}

impl ConsumerTask {
	pub fn is_finished(&self) -> bool {
		self.0.is_finished()
	}

	pub async fn stop(mut self) {
		self.0.abort();
		assert!(
			(&mut self.0)
				.await
				.as_ref()
				.is_err_and(|error| error.is_cancelled())
		);
	}
}

impl Drop for ConsumerTask {
	fn drop(&mut self) {
		self.0.abort();
	}
}
