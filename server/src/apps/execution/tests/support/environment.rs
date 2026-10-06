#[path = "postgres.rs"]
mod postgres;
use postgres::postgres_container;
use reinhardt::test::testcontainers::{
	ContainerAsync, GenericImage, ImageExt,
	core::{ContainerPort, WaitFor},
	runners::AsyncRunner,
};
use std::future::Future;
use std::{
	sync::{Arc, LazyLock, Weak},
	time::Duration,
};
use tokio::sync::Mutex;

pub const TEST_QDRANT_TOKEN: &str = "local-semantic-vector-fixture-key-0123456789";

static TEST_ENVIRONMENT: LazyLock<Mutex<Weak<TestEnvironment>>> =
	LazyLock::new(|| Mutex::new(Weak::new()));

/// Disposable service endpoints arranged by Testcontainers for integration tests.
///
/// The global cache stores only a weak reference: tests that overlap share one
/// service set, while the final fixture owner still triggers deterministic
/// Testcontainers cleanup.
#[derive(Debug)]
pub struct TestEnvironment {
	_postgres: ContainerAsync<GenericImage>,
	_qdrant: ContainerAsync<GenericImage>,
	_nats: ContainerAsync<GenericImage>,
	pub database_url: String,
	#[allow(dead_code)] // Only semantic integration-test binaries need Qdrant.
	pub qdrant_url: String,
	pub nats_url: String,
}

impl TestEnvironment {
	fn start() -> impl Future<Output = Self> {
		let postgres = Box::pin(postgres_container());
		async move {
			let (postgres, pool, _, database_url) = postgres.await;
			pool.close().await;
			let qdrant = GenericImage::new("qdrant/qdrant", "v1.19.1")
				.with_exposed_port(ContainerPort::Tcp(6333))
				.with_env_var("QDRANT__SERVICE__API_KEY", TEST_QDRANT_TOKEN)
				.start()
				.await
				.expect("start disposable Qdrant");
			let nats = GenericImage::new("nats", "2.12-alpine")
				.with_exposed_port(ContainerPort::Tcp(4222))
				.with_wait_for(WaitFor::message_on_stderr("Server is ready"))
				.with_cmd(["-js"])
				.start()
				.await
				.expect("start disposable NATS");
			let qdrant_url = format!(
				"http://{}:{}",
				qdrant.get_host().await.unwrap(),
				qdrant.get_host_port_ipv4(6333).await.unwrap()
			);
			let nats_url = format!(
				"nats://{}:{}",
				nats.get_host().await.unwrap(),
				nats.get_host_port_ipv4(4222).await.unwrap()
			);
			wait_for_nats(&nats_url).await;
			wait_for_qdrant(&qdrant_url).await;

			Self {
				_postgres: postgres,
				_qdrant: qdrant,
				_nats: nats,
				database_url,
				qdrant_url,
				nats_url,
			}
		}
	}
}

async fn wait_for_nats(url: &str) {
	for _ in 0..120 {
		if let Ok(Ok(client)) =
			tokio::time::timeout(Duration::from_secs(1), async_nats::connect(url)).await
			&& client.flush().await.is_ok()
		{
			return;
		}
		tokio::time::sleep(Duration::from_millis(250)).await;
	}
	panic!("NATS test container did not accept connections");
}

async fn wait_for_qdrant(url: &str) {
	let client = reqwest::Client::new();
	for _ in 0..120 {
		if client
			.get(format!("{url}/readyz"))
			.header("api-key", TEST_QDRANT_TOKEN)
			.send()
			.await
			.is_ok_and(|response| response.status().is_success())
		{
			return;
		}
		tokio::time::sleep(Duration::from_millis(250)).await;
	}
	panic!("Qdrant test container did not become ready");
}

#[rstest::fixture]
pub fn isolated_test_environment() -> impl Future<Output = Arc<TestEnvironment>> {
	let environment = Box::pin(TestEnvironment::start());
	async move { Arc::new(environment.await) }
}

#[rstest::fixture]
pub fn test_environment() -> impl Future<Output = Arc<TestEnvironment>> {
	let environment = Box::pin(TestEnvironment::start());
	async move {
		let mut cached = TEST_ENVIRONMENT.lock().await;
		if let Some(environment) = cached.upgrade() {
			return environment;
		}
		let environment = Arc::new(environment.await);
		*cached = Arc::downgrade(&environment);
		environment
	}
}
