use crate::endpoint::{EndpointFixture, endpoint};
use aidash_server::apps::execution::services::lifecycle::ProcessLifecycle;
use reinhardt::query::{Alias, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder};
use reinhardt::server::ShutdownCoordinator;
use rstest::rstest;
use std::time::Duration;

#[rstest]
#[tokio::test]
async fn readiness_bypasses_visibility_and_becomes_unavailable_while_draining(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let app = endpoint.await;
	let shutdown = ShutdownCoordinator::new(Duration::from_secs(3));
	ProcessLifecycle::register(&app.context, shutdown.clone());
	let mut held = app.database.connection.begin().await.unwrap();
	let sql = Query::select()
		.column(Alias::new("singleton"))
		.from(Alias::new("atomic_gate"))
		.lock(LockType::Update)
		.to_string(PostgresQueryBuilder);
	held.fetch_one(&sql, vec![]).await.unwrap();
	// Act
	let locked = app.anonymous.get("/ready").await.unwrap();
	shutdown.shutdown();
	let draining = app.anonymous.get("/ready").await.unwrap();
	let live = app.anonymous.get("/live").await.unwrap();
	// Assert
	assert_eq!(locked.status_code(), 200);
	assert_eq!(draining.status_code(), 503);
	assert_eq!(live.status_code(), 200);
	assert!(
		draining.body().is_empty(),
		"probe must not expose runtime details"
	);
	held.rollback().await.unwrap();
}

#[rstest]
#[tokio::test]
async fn database_outage_fails_readiness_without_failing_liveness(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let app = endpoint.await;
	assert_eq!(
		app.anonymous.get("/ready").await.unwrap().status_code(),
		200
	);
	// Act: close only this fixture's readiness/control pool.
	app.runtime.store.control_pool.close().await;
	let ready = app.anonymous.get("/ready").await.unwrap();
	let live = app.anonymous.get("/live").await.unwrap();
	// Assert
	assert_eq!(ready.status_code(), 503);
	assert_eq!(live.status_code(), 200);
	assert!(ready.body().is_empty());
}
