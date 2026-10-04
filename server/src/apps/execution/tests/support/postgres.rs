//! PostgreSQL fixture with the same JSON Schema extension as production.
use reinhardt::test::testcontainers::{
	ContainerAsync, GenericImage, ImageExt,
	core::{IntoContainerPort, WaitFor},
	runners::AsyncRunner,
};
use rstest::fixture;
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};

#[fixture]
pub async fn postgres_container() -> (ContainerAsync<GenericImage>, Arc<PgPool>, u16, String) {
	let image = GenericImage::new("aidash-orm-test-postgres", "17-pg-jsonschema-0.3.4")
		.with_exposed_port(5432.tcp())
		.with_wait_for(WaitFor::message_on_stderr(
			"database system is ready to accept connections",
		))
		.with_env_var("POSTGRES_USER", "aidash")
		.with_env_var("POSTGRES_PASSWORD", "fixture-password")
		.with_env_var("POSTGRES_DB", "aidash")
		.with_cmd(["postgres", "-c", "max_connections=400"])
		.with_startup_timeout(Duration::from_secs(120));
	let container = image.start().await.expect("build the test target of deploy/postgres/Dockerfile as aidash-orm-test-postgres:17-pg-jsonschema-0.3.4 before database tests");
	let port = container.get_host_port_ipv4(5432).await.unwrap();
	let url = format!("postgres://aidash:fixture-password@127.0.0.1:{port}/aidash");
	let pool = sqlx::postgres::PgPoolOptions::new()
		.max_connections(2)
		.acquire_timeout(Duration::from_secs(30))
		.connect(&url)
		.await
		.unwrap();
	(container, Arc::new(pool), port, url)
}
