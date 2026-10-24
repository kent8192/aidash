#[path = "postgres.rs"]
pub(super) mod postgres;
use futures_util::{
	FutureExt,
	future::{BoxFuture, Shared},
};
use postgres::{PostgresFuture, postgres_container};
use reinhardt::query::QueryStatementBuilder as _;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use reinhardt::test::fixtures::nats_container as native_nats_container;
use reinhardt::test::testcontainers::{ContainerAsync, GenericImage};
pub type EnvironmentFuture = Shared<BoxFuture<'static, Arc<TestEnvironment>>>;
use sqlx::Connection;
use std::{
	fmt::Display,
	future::Future,
	sync::{Arc, LazyLock, Weak},
	time::Duration,
};
use tokio::sync::Mutex;

static TEST_ENVIRONMENT: LazyLock<Mutex<Weak<TestEnvironment>>> =
	LazyLock::new(|| Mutex::new(Weak::new()));

/// One test step's bound. Healthy steps take seconds even under coverage
/// instrumentation, so a minute tells a hang from a slow runner long before
/// the CI job limit cancels the shard without output (#284, #308).
#[allow(dead_code)] // Only suites whose tests have hung bound their steps.
const STEP_BOUND: Duration = Duration::from_secs(60);

/// One shared fixture stage's bound. Provisioning and migrating a fresh
/// database with PGroonga is the slowest stage; it takes seconds, so three
/// minutes leaves room for coverage instrumentation on a loaded runner (#320).
const SETUP_BOUND: Duration = Duration::from_secs(180);

/// The bound for starting the shared PostgreSQL and NATS containers. It
/// exceeds PostgreSQL's own 120 s startup wait so that wait reports first.
const ENVIRONMENT_BOUND: Duration = Duration::from_secs(300);

/// A whole test's bound, for rstest's `#[timeout]`. It also covers fixture
/// resolution and teardown, which named steps cannot wrap.
#[allow(dead_code)] // Only suites whose tests have hung use the backstop.
pub const TEST_BOUND: Duration = Duration::from_secs(600);

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

	/// Await one named test step. A step that outlives [`STEP_BOUND`] fails
	/// with the PostgreSQL sessions and locks that can explain the wait.
	#[allow(dead_code)] // Only suites whose tests have hung bound their steps.
	pub async fn bounded<T>(&self, stage: impl Display, step: impl Future<Output = T>) -> T {
		self.within(STEP_BOUND, stage, step).await
	}

	/// Await one named stage of a shared fixture. Every test resolves these
	/// stages, so a hang anywhere in setup fails within [`SETUP_BOUND`] with
	/// the same PostgreSQL snapshot instead of silently holding the shard (#320).
	#[allow(dead_code)] // The commands suite includes this module without the shared fixtures.
	pub async fn bounded_setup<T>(&self, stage: impl Display, step: impl Future<Output = T>) -> T {
		self.within(SETUP_BOUND, stage, step).await
	}

	/// Await setup work that contains [`Self::bounded_setup`] stages of its
	/// own. Its bound is twice [`SETUP_BOUND`], so a nested stage that hangs
	/// still reports its own name first.
	#[allow(dead_code)] // The commands suite includes this module without the shared fixtures.
	pub async fn bounded_setup_around<T>(
		&self,
		stage: impl Display,
		step: impl Future<Output = T>,
	) -> T {
		self.within(SETUP_BOUND * 2, stage, step).await
	}

	async fn within<T>(
		&self,
		bound: Duration,
		stage: impl Display,
		step: impl Future<Output = T>,
	) -> T {
		match tokio::time::timeout(bound, step).await {
			Ok(output) => output,
			Err(_) => panic!(
				"{stage} did not finish within {bound:?}\n{}",
				self.activity().await
			),
		}
	}

	/// Describe the busy sessions and the waiting or advisory locks of the whole
	/// postmaster. DROP DATABASE and similar barriers make one database wait for
	/// another's sessions, so the snapshot is not limited to the test's database.
	async fn activity(&self) -> String {
		let sessions = Query::select()
			.expr(Expr::cust(
				"format('pid=%s database=%s application=%s state=%s wait=%s/%s \
				 blocked_by=%s transaction_age=%s query=%s', pid, datname, \
				 application_name, state, wait_event_type, wait_event, \
				 pg_blocking_pids(pid), now() - xact_start, left(query, 300))",
			))
			.from(Alias::new("pg_stat_activity"))
			// Background workers idle on an Activity wait. Idle clients matter
			// only while they still hold a session lock that blocks another.
			.and_where(Expr::cust(
				"pid <> pg_backend_pid() AND ((state IS DISTINCT FROM 'idle' \
				 AND wait_event_type IS DISTINCT FROM 'Activity') OR pid IN \
				 (SELECT unnest(pg_blocking_pids(pid)) FROM pg_stat_activity))",
			))
			.limit_literal(100)
			.to_string(PostgresQueryBuilder);
		let locks = Query::select()
			.expr(Expr::cust(
				"format('pid=%s %s %s granted=%s database=%s relation=%s key=%s/%s/%s', \
				 pid, locktype, mode, granted, database, relation, classid, objid, objsubid)",
			))
			.from(Alias::new("pg_locks"))
			.and_where(Expr::cust("NOT granted OR locktype = 'advisory'"))
			.limit_literal(100)
			.to_string(PostgresQueryBuilder);
		// A fresh superuser session sees every role's queries and does not wait
		// on a pool that the hung step may have exhausted.
		let snapshot = tokio::time::timeout(Duration::from_secs(10), async {
			let mut connection = sqlx::PgConnection::connect(&self.database_url).await?;
			let sessions: Vec<String> = sqlx::query_scalar(&sessions)
				.fetch_all(&mut connection)
				.await?;
			let locks: Vec<String> = sqlx::query_scalar(&locks)
				.fetch_all(&mut connection)
				.await?;
			connection.close().await?;
			Ok::<_, sqlx::Error>(format!(
				"PostgreSQL sessions:\n{}\nPostgreSQL waiting and advisory locks:\n{}",
				sessions.join("\n"),
				locks.join("\n")
			))
		})
		.await;
		match snapshot {
			Ok(Ok(snapshot)) => snapshot,
			Ok(Err(error)) => format!("PostgreSQL activity is unavailable: {error}"),
			Err(_) => "PostgreSQL activity is unavailable: the snapshot timed out".into(),
		}
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
		// The bound also covers waiting for another test's creation, so a
		// waiter fails within it instead of starting a second full attempt.
		let shared = async {
			let mut cached = TEST_ENVIRONMENT.lock().await;
			if let Some(environment) = cached.upgrade() {
				return environment;
			}
			let environment = environment.await;
			*cached = Arc::downgrade(&environment);
			environment
		};
		// No PostgreSQL session exists to describe yet; the stage name is the report.
		tokio::time::timeout(ENVIRONMENT_BOUND, shared)
			.await
			.unwrap_or_else(|_| {
				panic!(
					"start the shared PostgreSQL and NATS containers did not finish within \
					 {ENVIRONMENT_BOUND:?}"
				)
			})
	}
	.boxed()
	.shared()
}
