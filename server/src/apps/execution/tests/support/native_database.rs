use reinhardt::db::backends::DatabaseConnection as BackendConnection;
use reinhardt::db::migrations::{FilesystemSource, MigrationSource};
use reinhardt::db::orm::connection::DatabaseConnectionLease;
use reinhardt::query::QueryStatementBuilder as _;
#[path = "postgres.rs"]
mod postgres;
use futures_util::{
	FutureExt,
	future::{BoxFuture, Shared},
};
use postgres::PostgresFuture;
pub(super) use postgres::postgres_container;
pub type DatabaseFuture = Shared<BoxFuture<'static, DatabaseFixture>>;
use reinhardt::test::fixtures::temp_dir;
use reinhardt::test::testcontainers::{ContainerAsync, GenericImage};
use std::path::PathBuf;
use std::sync::Arc;
use sqlx::PgPool;
use std::time::Duration;
use tempfile::TempDir;

/// Trigger per-database crash recovery before a restarted Store uses PGroonga.
/// PostgreSQL's TCP readiness does not start this worker; see
/// https://pgroonga.github.io/reference/modules/pgroonga-crash-safer.html.
pub(super) async fn wait_for_pgroonga(pool: &PgPool, timeout: Duration) -> Result<(), String> {
	let probe = reinhardt::query::Query::select()
		.expr(reinhardt::query::SimpleExpr::FunctionCall(
			reinhardt::query::IntoIden::into_iden("pgroonga_command"),
			vec![reinhardt::query::Expr::value("status").into()],
		))
		.to_string(reinhardt::query::PostgresQueryBuilder);
	let mut last_error = None;
	let result = tokio::time::timeout(timeout, async {
		loop {
			// A failed initialization poisons that backend. Detach before probing
			// so even cancellation drops it rather than returning it to the pool.
			let mut session = pool.acquire().await?.detach();
			let result = sqlx::query(&probe).execute(&mut session).await;
			drop(session);
			match result {
				Ok(_) => break Ok(()),
				Err(sqlx::Error::Database(error))
					if error.code().as_deref() == Some("57P03")
						&& error
							.message()
							.contains("pgroonga_crash_safer is preparing") =>
				{
					last_error = Some(format!("SQLSTATE 57P03: {}", error.message()));
					tokio::time::sleep(Duration::from_millis(100)).await;
				}
				Err(error) => break Err(error),
			}
		}
	})
	.await;
	match result {
		Ok(Ok(())) => Ok(()),
		Ok(Err(error)) => Err(format!(
			"fixture PGroonga readiness probe failed: {error:?}"
		)),
		Err(_) => Err(format!(
			"fixture PGroonga did not become ready within {timeout:?}; last transient error: {}",
			last_error
				.as_deref()
				.unwrap_or("none (probe did not complete)")
		)),
	}
}

/// A native migration graph on Reinhardt's disposable PostgreSQL fixture.
#[allow(dead_code)] // Each integration binary consumes a different subset of the shared fixture.
#[derive(Clone)]
pub struct DatabaseFixture {
	pub lease: DatabaseConnectionLease,
	pub connection: BackendConnection,
	pub url: String,
	pub recovery_directory: Arc<tempfile::TempDir>,
	_container: Arc<ContainerAsync<GenericImage>>,
}

#[allow(dead_code)] // Shared by integration binaries without a crash test.
impl DatabaseFixture {
	/// Crash only this fixture's isolated server after committed writes.
	pub async fn kill_and_restart(
		&self,
		store: &aidash_server::store::Store,
	) -> aidash_server::store::Store {
		let status = tokio::process::Command::new("docker")
			.args(["kill", "--signal=KILL", self._container.id()])
			.stdout(std::process::Stdio::null())
			.status()
			.await
			.expect("kill this fixture PostgreSQL");
		assert!(status.success());
		self._container
			.start()
			.await
			.expect("restart this fixture PostgreSQL");
		// Docker can assign a new published port when a container starts again.
		// Reopen process-owned pools as an actual server restart would do.
		store.pool.close().await;
		store.control_pool.close().await;
		let port = self._container.get_host_port_ipv4(5432).await.unwrap();
		let url =
			format!("postgres://aidash:fixture-password@127.0.0.1:{port}/aidash?sslmode=disable");
		let pool = tokio::time::timeout(std::time::Duration::from_secs(30), async {
			loop {
				if let Ok(pool) = sqlx::postgres::PgPoolOptions::new()
					.max_connections(12)
					.acquire_timeout(std::time::Duration::from_secs(1))
					.connect(&url)
					.await
				{
					break pool;
				}
				tokio::time::sleep(std::time::Duration::from_millis(50)).await;
			}
		})
		.await
		.expect("crashed PostgreSQL must finish recovery");
		wait_for_pgroonga(&pool, Duration::from_secs(30))
			.await
			.unwrap_or_else(|error| panic!("{error}"));
		let restarted = aidash_server::store::Store::from_pool(pool, store.node_id.clone())
			.await
			.unwrap();
		aidash_server::semantic::services::memory_recovery::attach(
			restarted,
			self.recovery_directory.path().to_owned(),
		)
		.unwrap()
	}
}

#[rstest::fixture]
pub fn database(postgres_container: PostgresFuture, temp_dir: TempDir) -> DatabaseFuture {
	async move {
		let (container, pool, _port, url) = postgres_container.await;
		pool.close().await;
		let owner = BackendConnection::connect_postgres_with_pool_size(&url, Some(12))
			.await
			.expect("connect Reinhardt to the isolated test database");
		let migrations =
			FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
				.all_migrations()
				.await
				.expect("load generated migration sources");
		postgres::apply_migrations(owner.clone(), &migrations).await;
		let repeated =
			reinhardt::db::migrations::executor::DatabaseMigrationExecutor::new(owner.clone())
				.apply_migrations(&migrations)
				.await
				.expect("migrations are idempotent");
		assert!(repeated.applied.is_empty());
		DatabaseFixture {
			lease: DatabaseConnectionLease::register(owner.clone()).unwrap(),
			connection: owner,
			url,
			recovery_directory: Arc::new(temp_dir),
			_container: Arc::new(container),
		}
	}
	.boxed()
	.shared()
}
