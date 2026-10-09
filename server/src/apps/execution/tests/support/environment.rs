#[path = "postgres.rs"]
pub(super) mod postgres;
use futures_util::{
	FutureExt,
	future::{BoxFuture, Shared},
};
use postgres::{PostgresFuture, postgres_container};
use reinhardt::test::fixtures::nats_container as native_nats_container;
use reinhardt::test::testcontainers::{ContainerAsync, GenericImage};
pub type EnvironmentFuture = Shared<BoxFuture<'static, Arc<TestEnvironment>>>;
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
	#[allow(dead_code)] // NATS-only command targets do not consume the application broker URL.
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
}

/// Adapt the native Send JetStream fixture to the shared environment tuple.
pub type NatsFuture = BoxFuture<'static, (ContainerAsync<GenericImage>, String)>;
#[rstest::fixture]
pub fn nats_container(
	#[from(native_nats_container)] native_nats: impl std::future::Future<
		Output = (ContainerAsync<GenericImage>, u16, String),
	> + Send
	+ 'static,
) -> NatsFuture {
	// reinhardt-web#6705 fixes the native future Send contract tracked in #6702.
	Box::pin(async move {
		let (nats, _port, nats_url) = native_nats.await;
		(nats, nats_url)
	})
}

#[rstest::fixture]
pub fn isolated_test_environment(
	postgres_container: PostgresFuture,
	nats_container: NatsFuture,
) -> EnvironmentFuture {
	async move {
		let (postgres, pool, _, database_url) = postgres_container.await;
		pool.close().await;
		let (nats, nats_url) = nats_container.await;
		Arc::new(TestEnvironment {
			_postgres: postgres,
			_nats: nats,
			database_url,
			nats_url,
		})
	}
	.boxed()
	.shared()
}

#[rstest::fixture]
pub fn test_environment(isolated_test_environment: EnvironmentFuture) -> EnvironmentFuture {
	let environment = Box::pin(isolated_test_environment);
	async move {
		let mut cached = TEST_ENVIRONMENT.lock().await;
		if let Some(environment) = cached.upgrade() {
			return environment;
		}
		let environment = environment.await;
		*cached = Arc::downgrade(&environment);
		environment
	}
	.boxed()
	.shared()
}
