#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_server::apps::execution::services::lifecycle::ProcessLifecycle;

use futures_util::FutureExt;
use reinhardt::db::backends::{DatabaseConnection, dialect::PostgresBackend};
use reinhardt::db::migrations::{DatabaseMigrationRecorder, FilesystemSource, MigrationSource};
use reinhardt::server::ShutdownCoordinator;
use reinhardt::test::fixtures::api_client_from_url;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};

#[rstest]
#[tokio::test]
async fn simultaneous_replica_startup_reuses_the_preapplied_native_schema(
	#[from(common::runtime)] runtime_fixture: common::RuntimeFuture,
	#[future(awt)]
	#[from(migration_recorder)]
	#[with(runtime_fixture.clone())]
	recorder: DatabaseMigrationRecorder,
	#[future(awt)] migrations: Vec<reinhardt::db::migrations::Migration>,
) {
	// Arrange: deployment runs `manage migrate` before starting application replicas.
	// Retain the disposable environment through all simultaneous startup Acts.
	let runtime_owner = runtime_fixture.await;
	let (f, url, schema) = runtime_owner.parts();
	// Act: simultaneous startup is the lifecycle operation under test.
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
	#[future(awt)] probe_fixture: ProbeFixture,
) {
	// Arrange
	let (f, url, schema) = probe_fixture.application.runtime.parts();
	let app = probe_fixture.application.application.clone();
	let ProbeFixture {
		stop,
		probe,
		client,
		..
	} = probe_fixture;
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

#[fixture]
async fn migration_recorder(
	#[future(awt)]
	#[from(common::runtime)]
	runtime_fixture: common::RuntimeFixture,
) -> DatabaseMigrationRecorder {
	DatabaseMigrationRecorder::new(DatabaseConnection::new(Arc::new(PostgresBackend::new(
		runtime_fixture.federation.store.pool.driver().clone(),
	))))
}
#[fixture]
async fn migrations() -> Vec<reinhardt::db::migrations::Migration> {
	FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
		.all_migrations()
		.await
		.unwrap()
}
#[fixture]
fn shutdown() -> ShutdownCoordinator {
	ShutdownCoordinator::new(Duration::from_secs(3))
}

type ProbeContextFuture = futures_util::future::Shared<
	futures_util::future::BoxFuture<'static, (common::ApplicationFixture, ShutdownCoordinator)>,
>;
type ProbeServerFuture = futures_util::future::Shared<
	futures_util::future::BoxFuture<
		'static,
		Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	>,
>;
type ProbeClientFuture = futures_util::future::Shared<
	futures_util::future::BoxFuture<'static, Arc<reinhardt::test::APIClient>>,
>;
#[fixture]
fn probe_context(
	#[from(common::native_application)] application: common::ApplicationFuture,
	shutdown: ShutdownCoordinator,
) -> ProbeContextFuture {
	async move {
		let application = application.await;
		ProcessLifecycle::register(&application.application.context, shutdown.clone());
		(application, shutdown)
	}
	.boxed()
	.shared()
}
#[fixture]
fn probe_server(probe_context: ProbeContextFuture) -> ProbeServerFuture {
	async move {
		let (application, _) = probe_context.await;
		// reinhardt-web#6658: the owning fixture bridges the pinned guard API.
		// reinhardt-web#6673: serve production probe routes directly to preserve HEAD.
		Arc::new(
			reinhardt::test::fixtures::server::test_server_guard(
				aidash_server::apps::execution::urls::probe_url_patterns()
					.with_di_context(application.application.context.clone()),
			)
			.await,
		)
	}
	.boxed()
	.shared()
}
#[fixture]
fn probe_client(probe_server: ProbeServerFuture) -> ProbeClientFuture {
	async move {
		let server = probe_server.await;
		// reinhardt-web#6658: compose the plain client constructor over the owned guard future.
		Arc::new(api_client_from_url(&server.url))
	}
	.boxed()
	.shared()
}
struct ProbeFixture {
	application: common::ApplicationFixture,
	stop: ShutdownCoordinator,
	probe: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	client: Arc<reinhardt::test::APIClient>,
}
#[fixture]
async fn probe_fixture(
	probe_context: ProbeContextFuture,
	#[from(probe_server)]
	#[with(probe_context.clone())]
	server: ProbeServerFuture,
	#[from(probe_client)]
	#[with(server.clone())]
	client: ProbeClientFuture,
) -> ProbeFixture {
	let (application, stop) = probe_context.await;
	ProbeFixture {
		application,
		stop,
		probe: server.await,
		client: client.await,
	}
}
