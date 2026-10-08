#[path = "postgres.rs"]
pub(super) mod postgres;
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
	_nats: ContainerAsync<GenericImage>,
	pub database_url: String,
	pub nats_url: String,
}

impl TestEnvironment {
	#[allow(dead_code)]
	pub async fn restart_database(&self) {
		self._postgres.stop_with_timeout(Some(1)).await.unwrap();
		self._postgres.start().await.unwrap();
		let ready = tokio::time::timeout(Duration::from_secs(60), async {
			loop {
				if let Ok(pool) = sqlx::postgres::PgPoolOptions::new()
					.max_connections(1)
					.acquire_timeout(Duration::from_secs(1))
					.connect(&self.database_url)
					.await
				{
					pool.close().await;
					break;
				}
				tokio::time::sleep(Duration::from_millis(100)).await;
			}
		})
		.await;
		ready.expect("restarted PostgreSQL must accept connections");
	}
	fn start() -> impl Future<Output = Self> {
		let postgres = Box::pin(postgres_container());
		async move {
			let (postgres, pool, _, database_url) = postgres.await;
			pool.close().await;
			let nats = GenericImage::new("nats", "2.12-alpine")
				.with_exposed_port(ContainerPort::Tcp(4222))
				.with_wait_for(WaitFor::message_on_stderr("Server is ready"))
				.with_cmd(["-js"])
				.start()
				.await
				.expect("start disposable NATS");
			let nats_url = format!(
				"nats://{}:{}",
				nats.get_host().await.unwrap(),
				nats.get_host_port_ipv4(4222).await.unwrap()
			);
			wait_for_nats(&nats_url).await;

			Self {
				_postgres: postgres,
				_nats: nats,
				database_url,
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
