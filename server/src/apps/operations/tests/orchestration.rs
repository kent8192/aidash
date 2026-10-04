#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_server::apps::execution::services::lifecycle::ProcessLifecycle;
use common::{TestEnvironment, test_environment};
use reinhardt::db::backends::{DatabaseConnection, dialect::PostgresBackend};
use reinhardt::db::migrations::{DatabaseMigrationRecorder, FilesystemSource, MigrationSource};
use reinhardt::server::ShutdownCoordinator;
use reinhardt::test::fixtures::{api_client_from_url, server::test_server_guard};
use rstest::rstest;
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};

#[rstest]
#[tokio::test]
async fn simultaneous_replica_startup_reuses_the_preapplied_native_schema(
	#[future(awt)] test_environment: Arc<TestEnvironment>,
) {
	// Arrange: deployment runs `manage migrate` before starting application replicas.
	let (f, url, schema) = common::setup(&test_environment).await;
	let recorder = DatabaseMigrationRecorder::new(DatabaseConnection::new(Arc::new(
		PostgresBackend::new(f.store.pool.clone()),
	)));
	let migrations =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await
			.unwrap();
	// Act
	let (a, b, c) = tokio::join!(
		common::application(f.clone()),
		common::application(f.clone()),
		common::application(f.clone())
	);
	for app in [&a, &b, &c] {
		let (status, workspace) = common::request(
			app,
			&f.config.api_token,
			"POST",
			"/api/workspaces",
			json!({"title":"Replica startup", "goal":"Use the migrated schema"}),
		)
		.await;
		assert_eq!(status, 200, "{workspace}");
	}
	// Assert: startup neither creates another history nor reapplies schema operations.
	let applied = recorder.get_applied_migrations().await.unwrap();
	let expected: BTreeSet<_> = migrations
		.iter()
		.map(|m| (m.app_label.as_str(), m.name.as_str()))
		.collect();
	let actual: BTreeSet<_> = applied
		.iter()
		.map(|m| (m.app.as_str(), m.name.as_str()))
		.collect();
	assert_eq!(actual, expected);
	assert_eq!(applied.len(), migrations.len());
	assert_eq!(f.store.workspaces().await.unwrap().len(), 3);
	common::cleanup(f, &url, &schema).await;
}

#[rstest]
#[tokio::test]
async fn probes_report_draining_without_restarting_for_dependency_failure(
	#[future(awt)] test_environment: Arc<TestEnvironment>,
) {
	// Arrange
	let (f, url, schema) = common::setup(&test_environment).await;
	let app = common::application(f.clone()).await;
	let (_, subject, _) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	assert_eq!(
		common::request(&app, &subject, "GET", "/api/deployment", Value::Null)
			.await
			.0,
		403
	);
	assert_eq!(
		common::request(
			&app,
			&f.config.api_token,
			"GET",
			"/api/deployment",
			Value::Null
		)
		.await
		.1["enabled"],
		false
	);
	let stop = ShutdownCoordinator::new(Duration::from_secs(3));
	ProcessLifecycle::register(&app.context, stop.clone());
	let probe = test_server_guard(
		aidash_server::apps::execution::urls::probe_url_patterns()
			.with_di_context(app.context.clone()),
	)
	.await;
	let client = api_client_from_url(&probe.url);
	assert_eq!(client.get("/ready").await.unwrap().status_code(), 200);
	// Act
	f.store.control_pool.close().await;
	let unavailable = client.get("/ready").await.unwrap();
	stop.shutdown();
	let draining = client.get("/ready").await.unwrap();
	let live = client.get("/live").await.unwrap();
	// Assert
	assert_eq!(unavailable.status_code(), 503);
	assert_eq!(draining.status_code(), 503);
	assert_eq!(live.status_code(), 200);
	assert_eq!(client.get("/api/state").await.unwrap().status_code(), 404);
	drop(probe);
	common::cleanup(f, &url, &schema).await;
}
