use crate::endpoint::{EndpointFixture, endpoint};
use aidash_server::{Result, bus::EventBus};
use reinhardt::test::testcontainers::{
	ContainerAsync, GenericImage, ImageExt,
	core::{ContainerPort, WaitFor},
	runners::AsyncRunner,
};
use rstest::fixture;
use std::future::Future;
use tokio::task::JoinHandle;

pub struct BusFixture {
	pub app: EndpointFixture,
	pub bus: EventBus,
	_broker: ContainerAsync<GenericImage>,
}

#[fixture]
pub fn event_bus(#[future] endpoint: EndpointFixture) -> impl Future<Output = BusFixture> {
	let endpoint = Box::pin(endpoint);
	async move {
		let broker = GenericImage::new("nats", "2.12-alpine")
			.with_exposed_port(ContainerPort::Tcp(4222))
			.with_wait_for(WaitFor::message_on_stderr("Server is ready"))
			.with_cmd(["-js"])
			.start()
			.await
			.unwrap();
		let app = endpoint.await;
		let port = broker.get_host_port_ipv4(4222).await.unwrap();
		let host = broker.get_host().await.unwrap();
		let bus = EventBus::connect(
			&format!("nats://{host}:{port}"),
			&app.runtime.config.node_id,
		)
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
