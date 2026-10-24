//! PostgreSQL fixture with the same JSON Schema extension as production.
use reinhardt::query::QueryStatementBuilder as _;
use reinhardt::test::fixtures::{PostgresContainerConfig, postgres_container_with};
use reinhardt::test::testcontainers::{ContainerAsync, GenericImage, core::WaitFor};
use rstest::fixture;
use sqlx::{Connection, PgPool};
use std::{sync::Arc, time::Duration};
pub type PostgresFuture = std::pin::Pin<
	Box<
		dyn std::future::Future<Output = (ContainerAsync<GenericImage>, Arc<PgPool>, u16, String)>
			+ Send,
	>,
>;

#[fixture]
fn postgres_config() -> (PostgresContainerConfig, std::net::TcpListener) {
	// Preserve the restart mapping outside the ephemeral outbound connection range.
	let reservation = (0..128)
		.find_map(|_| {
			let port = 10_000 + (uuid::Uuid::new_v4().as_u128() % 20_000) as u16;
			std::net::TcpListener::bind(("0.0.0.0", port)).ok()
		})
		.expect("reserve an available PostgreSQL fixture listener port");
	let host_port = reservation.local_addr().unwrap().port();
	let config = PostgresContainerConfig::default()
        .image("aidash-orm-test-postgres", "17-pg-jsonschema-0.3.4")
        .user(ADMIN.0)
        .password(ADMIN.1)
        .database("aidash")
        .host_port(host_port)
        // Ignore the temporary Unix-only initialization server.
        .wait_for(WaitFor::message_on_stderr(
            "listening on IPv4 address \"0.0.0.0\", port 5432",
        ))
        .args(["postgres", "-c", "max_connections=400", "-c", "max_worker_processes=256"])
        .startup_timeout(Duration::from_secs(120));
	(config, reservation)
}

#[fixture]
pub fn postgres_container(
	postgres_config: (PostgresContainerConfig, std::net::TcpListener),
) -> PostgresFuture {
	Box::pin(async move {
		let (config, reservation) = postgres_config;
		drop(reservation);
		// The native configurable fixture retains the custom PostgreSQL 17 image.
		let (container, pool, port, url) = postgres_container_with(config).await;
		(container, pool, port, url)
	})
}

/// The container's bootstrap superuser, used only to provision fixtures.
pub const ADMIN: (&str, &str) = ("aidash", "fixture-password");
/// The owner login that runs migrations and owns every table (#262).
pub const OWNER: (&str, &str) = ("aidash_owner", "fixture-owner-password");
/// The server login, which row-level security binds and which holds only DML.
pub const SERVER: (&str, &str) = ("aidash_server", "fixture-server-password");

/// Logins to one fixture database, provisioned like a deployment's.
#[allow(dead_code)] // Shared integration binaries use different fixture constructors.
#[derive(Clone, Debug)]
pub struct FixtureDatabase {
	pub owner_url: String,
	pub server_url: String,
}

/// The same database URL with another login.
#[allow(dead_code)]
pub fn with_login(url: &str, (user, password): (&str, &str)) -> String {
	let mut url = reqwest::Url::parse(url).unwrap();
	url.set_username(user).unwrap();
	url.set_password(Some(password)).unwrap();
	url.to_string()
}

/// Whether a fixture database runs PGroonga with crash-safe recovery, as every
/// deployment does.
#[allow(dead_code)] // Shared integration binaries use different fixture constructors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PgroongaRecovery {
	CrashSafe,
	/// Each crash-safe database keeps one PGroonga flush worker until it is
	/// dropped. A container that keeps every database runs out of worker slots,
	/// and the next `CREATE EXTENSION pgroonga` then waits forever (#381).
	Off,
}

/// Create a database owned by the owner role, with the administrator's
/// extensions and the server role's default privileges, before any migration.
///
/// Role and database administration have no Reinhardt Query builder; this is
/// fixture DDL issued as the container's superuser. Default privileges are
/// per database and apply only to objects created afterwards.
#[allow(dead_code)]
pub async fn prepare_database(
	admin_url: &str,
	database: &str,
	recovery: PgroongaRecovery,
) -> FixtureDatabase {
	assert!(
		database
			.bytes()
			.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
	);
	let admin = sqlx::postgres::PgPoolOptions::new()
		.max_connections(1)
		.connect(admin_url)
		.await
		.unwrap();
	// Parallel test processes share one container: tolerate a concurrent creation.
	for (role, password) in [OWNER, SERVER] {
		sqlx::raw_sql(&format!(
			"DO $$ BEGIN CREATE ROLE {role} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS PASSWORD '{password}'; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"
		))
		.execute(&admin)
		.await
		.unwrap();
	}
	sqlx::raw_sql(&format!("CREATE DATABASE {database} OWNER {}", OWNER.0))
		.execute(&admin)
		.await
		.unwrap();
	if recovery == PgroongaRecovery::Off {
		sqlx::raw_sql(&format!(
			"ALTER DATABASE {database} SET pgroonga.enable_crash_safe = off"
		))
		.execute(&admin)
		.await
		.unwrap();
	}
	admin.close().await;
	let mut url = reqwest::Url::parse(admin_url).unwrap();
	url.set_path(database);
	let admin_url = url.to_string();
	let admin = sqlx::postgres::PgPoolOptions::new()
		.max_connections(1)
		.connect(&admin_url)
		.await
		.unwrap();
	sqlx::raw_sql(&format!(
		"CREATE EXTENSION pg_jsonschema WITH SCHEMA public; CREATE EXTENSION vector; \
		 ALTER DEFAULT PRIVILEGES FOR ROLE {owner} GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO {server}; \
		 ALTER DEFAULT PRIVILEGES FOR ROLE {owner} GRANT USAGE, SELECT ON SEQUENCES TO {server};",
		owner = OWNER.0,
		server = SERVER.0,
	))
	.execute(&admin)
	.await
	.unwrap();
	prepare_pgroonga(&admin).await;
	admin.close().await;
	FixtureDatabase {
		owner_url: with_login(&admin_url, OWNER),
		server_url: with_login(&admin_url, SERVER),
	}
}

/// A small pool on the same database as the owner, for fixture-only DDL such
/// as fault injection. The server role cannot change the schema.
#[allow(dead_code)]
pub async fn owner_pool(pool: &PgPool) -> PgPool {
	login_pool(pool, OWNER).await
}

/// A small pool on the same database as the container's superuser, for
/// fixture-only instrumentation that even the owner cannot install.
#[allow(dead_code)]
pub async fn admin_pool(pool: &PgPool) -> PgPool {
	login_pool(pool, ADMIN).await
}

async fn login_pool(pool: &PgPool, (user, password): (&str, &str)) -> PgPool {
	let options = pool
		.connect_options()
		.as_ref()
		.clone()
		.username(user)
		.password(password);
	sqlx::postgres::PgPoolOptions::new()
		.max_connections(2)
		.connect_with(options)
		.await
		.unwrap()
}

/// PGroonga's per-database crash-safe worker starts asynchronously on first use.
/// Only its documented preparing state is retryable; permanent errors surface.
#[allow(dead_code)] // Shared integration binaries use different migration constructors.
pub async fn apply_migrations(
	connection: reinhardt::db::backends::DatabaseConnection,
	migrations: &[reinhardt::db::migrations::Migration],
) {
	let pool = connection.clone().into_postgres().unwrap();
	prepare_pgroonga(&pool).await;
	let started = std::time::Instant::now();
	loop {
		match reinhardt::db::migrations::executor::DatabaseMigrationExecutor::new(
			connection.clone(),
		)
		.apply_migrations(migrations)
		.await
		{
			Ok(_) => return,
			Err(error)
				if error
					.to_string()
					.contains("pgroonga_crash_safer is preparing")
					&& started.elapsed() < Duration::from_secs(30) =>
			{
				// PGroonga remembers a failed initialization in that backend.
				// Retry on fresh sessions rather than reusing a poisoned pool slot.
				let pool = connection.clone().into_postgres().unwrap();
				for _ in 0..pool.size() {
					pool.acquire()
						.await
						.unwrap()
						.detach()
						.close()
						.await
						.unwrap();
				}
				tokio::time::sleep(Duration::from_millis(100)).await;
			}
			Err(error) => panic!("apply native migrations to the isolated database: {error}"),
		}
	}
}

/// Create PGroonga if it is missing, then flush its initial catalog.
async fn prepare_pgroonga(pool: &PgPool) {
	// Extension creation commits before the first PGroonga feature starts its
	// per-database recovery worker. Flush its initial catalog before another
	// backend opens the database for index DDL, following
	// https://pgroonga.github.io/reference/modules/pgroonga-crash-safer.html.
	// CREATE EXTENSION is administrative fixture DDL unsupported by SeaQuery.
	let probe = reinhardt::query::Query::select()
		.expr(reinhardt::query::SimpleExpr::FunctionCall(
			reinhardt::query::IntoIden::into_iden("pgroonga_command"),
			vec![reinhardt::query::Expr::value("io_flush").into()],
		))
		.to_string(reinhardt::query::PostgresQueryBuilder);
	let started = std::time::Instant::now();
	loop {
		let mut session = pool.acquire().await.unwrap().detach();
		let result = async {
			sqlx::query("CREATE EXTENSION IF NOT EXISTS pgroonga")
				.execute(&mut session)
				.await?;
			sqlx::query(&probe).execute(&mut session).await?;
			Ok::<(), sqlx::Error>(())
		}
		.await;
		session.close().await.unwrap();
		match result {
			Ok(()) => break,
			Err(error)
				if error
					.to_string()
					.contains("pgroonga_crash_safer is preparing")
					&& started.elapsed() < Duration::from_secs(30) =>
			{
				tokio::time::sleep(Duration::from_millis(100)).await;
			}
			Err(error) => panic!("initialize fixture PGroonga recovery worker: {error}"),
		}
	}
}
