use reinhardt::db::backends::DatabaseConnection as BackendConnection;
use reinhardt::db::migrations::executor::DatabaseMigrationExecutor;
use reinhardt::db::migrations::{FilesystemSource, MigrationSource};
use reinhardt::db::orm::connection::DatabaseConnectionLease;
#[path = "postgres.rs"]
mod postgres;
use postgres::postgres_container;
use reinhardt::test::testcontainers::{ContainerAsync, GenericImage};
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;

/// A native migration graph on Reinhardt's disposable PostgreSQL fixture.
#[allow(dead_code)] // Each integration binary consumes a different subset of the shared fixture.
pub struct DatabaseFixture {
	pub lease: DatabaseConnectionLease,
	pub connection: BackendConnection,
	pub url: String,
	_container: ContainerAsync<GenericImage>,
}

#[rstest::fixture]
pub async fn database(
	#[future] postgres_container: (ContainerAsync<GenericImage>, Arc<PgPool>, u16, String),
) -> DatabaseFixture {
	let (container, pool, _port, url) = postgres_container.await;
	pool.close().await;
	let owner = BackendConnection::connect_postgres_with_pool_size(&url, Some(12))
		.await
		.expect("connect Reinhardt to the isolated test database");
	let mut executor = DatabaseMigrationExecutor::new(owner.clone());
	let migrations =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await
			.expect("load generated migration sources");
	executor
		.apply_migrations(&migrations)
		.await
		.expect("apply fresh migration graph");
	let repeated = executor
		.apply_migrations(&migrations)
		.await
		.expect("migrations are idempotent");
	assert!(repeated.applied.is_empty());
	DatabaseFixture {
		lease: DatabaseConnectionLease::register(owner.clone()).unwrap(),
		connection: owner,
		url,
		_container: container,
	}
}
