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
#[allow(unused_imports)] // Only fault-injection suites change the schema.
pub use postgres::{ADMIN, OWNER, admin_pool, owner_pool, with_login};
pub type DatabaseFuture = Shared<BoxFuture<'static, DatabaseFixture>>;
use reinhardt::test::fixtures::temp_dir;
use reinhardt::test::testcontainers::{ContainerAsync, GenericImage};
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;
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
	/// The server role's connection, which row-level security binds.
	pub connection: BackendConnection,
	/// The owner's connection, for migrations and fixture-only DDL.
	pub owner_connection: BackendConnection,
	/// The server role's URL.
	pub url: String,
	/// The owner's URL, as a deployment's migration Job receives it.
	pub owner_url: String,
	pub recovery_directory: Arc<tempfile::TempDir>,
	_container: Arc<ContainerAsync<GenericImage>>,
}

#[allow(dead_code)] // Shared by integration binaries without a crash test.
impl DatabaseFixture {
	/// A pool on the server role, for code that opens scoped transactions.
	pub async fn pool(&self) -> aidash_server::database::native::Pool {
		aidash_server::database::native::Pool::from(
			aidash_server::database::runtime::connect(&self.url, 4)
				.await
				.unwrap()
				.into_postgres()
				.unwrap(),
		)
	}

	/// Run ORM `work` in the test-fixture Deployment Scope, for fixtures that
	/// seed or inspect rows of tables with row-level policies.
	pub async fn fixture<T>(
		&self,
		work: impl for<'t> AsyncFnOnce(
			&'t mut reinhardt::db::orm::AtomicTransaction,
		) -> aidash_server::Result<T>,
	) -> T {
		self.server_pool()
			.atomic(&aidash_application::tenancy::TEST_FIXTURE, work)
			.await
			.unwrap()
	}

	/// Write the Tenant root record `name`, which every row of that Tenant
	/// references, and return the Tenant's scope.
	pub async fn tenant(&self, name: &str) -> aidash_application::tenancy::TenancyScope {
		use reinhardt::query::{
			Alias, Expr, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
		};
		let insert = Query::insert()
			.into_table(Alias::new("tenants"))
			.columns(["name", "personal", "created_at", "created_by_kind"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(name))
					.expr(Expr::value(false))
					.expr(Expr::cust("clock_timestamp()"))
					.expr(Expr::value("deployment_administrator"))
					.to_owned(),
			)
			.on_conflict(
				OnConflict::column(Alias::new("name"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder);
		let mut tx = self.begin_fixture().await;
		aidash_server::database::native::query(&insert)
			.execute(&mut *tx)
			.await
			.unwrap();
		tx.commit().await.unwrap();
		aidash_application::tenancy::TenancyScope::tenant(name).unwrap()
	}

	/// Begin a transaction in the test-fixture Deployment Scope.
	pub async fn begin_fixture(&self) -> aidash_server::database::native::Transaction {
		self.begin_in(&aidash_application::tenancy::TEST_FIXTURE)
			.await
	}

	/// Begin a transaction in `scope`, as the server would for the same work.
	pub async fn begin_in(
		&self,
		scope: &aidash_application::tenancy::TenancyScope,
	) -> aidash_server::database::native::Transaction {
		self.server_pool().begin(scope).await.unwrap()
	}

	fn server_pool(&self) -> aidash_server::database::native::Pool {
		aidash_server::database::native::Pool::from(
			self.connection.clone().into_postgres().unwrap(),
		)
	}

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
		let mut url = reqwest::Url::parse(&self.url).unwrap();
		url.set_port(Some(port)).unwrap();
		let url = url.to_string();
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
		let mut restarted = aidash_server::store::Store::from_pool(pool, store.node_id.clone())
			.await
			.unwrap();
		// Deployment configuration survives a crash; only process state is lost.
		restarted.keyless = store.keyless.clone();
		aidash_server::semantic::services::memory_recovery::attach(
			restarted,
			self.recovery_directory.path().to_owned(),
		)
		.await
		.unwrap()
	}
}

#[rstest::fixture]
pub fn database(postgres_container: PostgresFuture, temp_dir: TempDir) -> DatabaseFuture {
	async move {
		let (container, pool, _port, url) = postgres_container.await;
		pool.close().await;
		let logins =
			postgres::prepare_database(&url, "native", postgres::PgroongaRecovery::CrashSafe).await;
		let owner = BackendConnection::connect_postgres_with_pool_size(&logins.owner_url, Some(4))
			.await
			.expect("connect Reinhardt to the isolated test database as its owner");
		// Community's migrations, then those of the build the test installed.
		let community =
			PathBuf::from(aidash_server::bootstrap::build::Build::COMMUNITY.manifest_dir);
		let mut migrations = Vec::new();
		for root in aidash_server::bootstrap::build::current().migration_roots(&community) {
			migrations.extend(
				FilesystemSource::new(root)
					.all_migrations()
					.await
					.expect("load generated migration sources"),
			);
		}
		postgres::apply_migrations(owner.clone(), &migrations).await;
		let repeated =
			reinhardt::db::migrations::executor::DatabaseMigrationExecutor::new(owner.clone())
				.apply_migrations(&migrations)
				.await
				.expect("migrations are idempotent");
		assert!(repeated.applied.is_empty());
		let connection =
			BackendConnection::connect_postgres_with_pool_size(&logins.server_url, Some(12))
				.await
				.expect("connect Reinhardt to the isolated test database as the server");
		// The lease is the fixture's own handle for seeding and inspecting rows of
		// every Tenant, so its sessions hold the test-fixture Deployment Scope.
		// The connection stays strict: server code under test opens its own scopes.
		let fixture_pool = aidash_server::database::scope::test_fixture_driver(
			std::str::FromStr::from_str(&logins.server_url).expect("the server URL parses"),
			4,
		);
		let fixture_connection =
			aidash_server::database::native::Pool::from(fixture_pool).connection();
		DatabaseFixture {
			lease: DatabaseConnectionLease::register(fixture_connection).unwrap(),
			connection,
			owner_connection: owner,
			url: logins.server_url,
			owner_url: logins.owner_url,
			recovery_directory: Arc::new(temp_dir),
			_container: Arc::new(container),
		}
	}
	.boxed()
	.shared()
}
