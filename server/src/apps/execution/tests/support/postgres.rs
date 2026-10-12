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
        .user("aidash")
        .password("fixture-password")
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

/// PGroonga's per-database crash-safe worker starts asynchronously on first use.
/// Only its documented preparing state is retryable; permanent errors surface.
#[allow(dead_code)] // Shared integration binaries use different migration constructors.
pub async fn apply_migrations(
	connection: reinhardt::db::backends::DatabaseConnection,
	migrations: &[reinhardt::db::migrations::Migration],
) {
	// Extension creation commits before the first PGroonga feature starts its
	// per-database recovery worker. Flush its initial catalog before another
	// backend opens the database for index DDL, following
	// https://pgroonga.github.io/reference/modules/pgroonga-crash-safer.html.
	// CREATE EXTENSION is administrative fixture DDL unsupported by SeaQuery.
	let pool = connection.clone().into_postgres().unwrap();
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
