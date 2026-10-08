//! PostgreSQL fixture with the same JSON Schema extension as production.
use reinhardt::query::QueryStatementBuilder as _;
use reinhardt::test::testcontainers::{
	ContainerAsync, GenericImage, ImageExt,
	core::{IntoContainerPort, WaitFor},
	runners::AsyncRunner,
};
use rstest::fixture;
use sqlx::{Connection, PgPool};
use std::{sync::Arc, time::Duration};

#[fixture]
pub async fn postgres_container() -> (ContainerAsync<GenericImage>, Arc<PgPool>, u16, String) {
	// Keep an explicit mapping across stop/start. Avoid the usual ephemeral range:
	// outbound fixture connections can claim those ports after the reservation drops.
	let reservation = (0..128)
		.find_map(|_| {
			let port = 10_000 + (uuid::Uuid::new_v4().as_u128() % 20_000) as u16;
			std::net::TcpListener::bind(("0.0.0.0", port)).ok()
		})
		.expect("reserve an available PostgreSQL fixture listener port");
	let host_port = reservation.local_addr().unwrap().port();
	let image = GenericImage::new("aidash-orm-test-postgres", "17-pg-jsonschema-0.3.4")
		.with_exposed_port(5432.tcp())
		// The initialization server only accepts Unix sockets. Wait for the final
		// TCP listener, then verify readiness with an actual pool connection below.
		.with_wait_for(WaitFor::message_on_stderr(
			"listening on IPv4 address \"0.0.0.0\", port 5432",
		))
		.with_env_var("POSTGRES_USER", "aidash")
		.with_env_var("POSTGRES_PASSWORD", "fixture-password")
		.with_env_var("POSTGRES_DB", "aidash")
		.with_mapped_port(host_port, 5432.tcp())
		.with_cmd([
			"postgres",
			"-c",
			"max_connections=400",
			"-c",
			"max_worker_processes=256",
		])
		.with_startup_timeout(Duration::from_secs(120));
	drop(reservation);
	let container = image.start().await.expect("build the test target of deploy/postgres/Dockerfile as aidash-orm-test-postgres:17-pg-jsonschema-0.3.4 before database tests");
	let port = container.get_host_port_ipv4(5432).await.unwrap();
	// The fixture has no TLS listener; avoid negotiating through an initializing
	// Docker port proxy before its PostgreSQL backend becomes available.
	let url = format!("postgres://aidash:fixture-password@127.0.0.1:{port}/aidash?sslmode=disable");
	let pool = tokio::time::timeout(Duration::from_secs(30), async {
		loop {
			match sqlx::postgres::PgPoolOptions::new()
				.max_connections(2)
				.acquire_timeout(Duration::from_secs(1))
				.connect(&url)
				.await
			{
				Ok(pool) => break pool,
				Err(sqlx::Error::Io(_) | sqlx::Error::PoolTimedOut | sqlx::Error::Protocol(_)) => {}
				Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("57P03") => {}
				Err(error) => panic!("fixture PostgreSQL connection failed: {error}"),
			}
			tokio::time::sleep(Duration::from_millis(50)).await;
		}
	})
	.await
	.expect("fixture PostgreSQL must accept TCP connections within 30 seconds");
	(container, Arc::new(pool), port, url)
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
