use crate::manage::{ManageFixture, native_server, native_worker};
use aidash_server::apps::workspaces::models::Workspace;
use reinhardt::db::orm::Model;
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;

#[rstest]
#[tokio::test]
async fn manage_runserver_initializes_routes_authentication_and_persistence(
	#[future] native_server: ManageFixture,
) {
	// Arrange
	let mut app = native_server.await;
	// Act
	let response = app
		.client
		.post(
			"/api/workspaces",
			&json!({
				"title":"Native startup", "goal":"Exercise the real management binary"
			}),
			"json",
		)
		.await
		.unwrap();
	assert_eq!(response.status_code(), 200, "{}", response.text());
	let identity = app
		.client
		.get("/.well-known/aidash")
		.await
		.unwrap()
		.json_value()
		.unwrap();
	let body = response.json_value().unwrap();
	let id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
	let workspace = Workspace::objects()
		.filter(Workspace::field_id().eq(id))
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	// Assert
	assert_eq!(workspace.title, "Native startup");
	assert_eq!(identity["id"], "aidash://manage-fixture");
	assert_eq!(
		app.client
			.get(&format!("/api/workspaces/{id}?token=fixture-private-query"))
			.await
			.unwrap()
			.status_code(),
		200
	);
	assert_eq!(app.probes.get("/live").await.unwrap().status_code(), 200);
	assert_eq!(app.probes.get("/ready").await.unwrap().status_code(), 200);
	assert_eq!(
		app.probes.get("/api/state").await.unwrap().status_code(),
		404
	);
	let metrics = app.metrics.get("/metrics").await.unwrap();
	assert_eq!(metrics.status_code(), 200);
	let metrics = metrics.text();
	for (name, labels, value) in [
		(
			"axum_http_requests_total",
			[
				"method=\"POST\"",
				"endpoint=\"/api/workspaces\"",
				"status=\"200\"",
			]
			.as_slice(),
			" 1",
		),
		(
			"axum_http_requests_total",
			[
				"method=\"GET\"",
				"endpoint=\"/api/workspaces/{id}\"",
				"status=\"200\"",
			]
			.as_slice(),
			" 1",
		),
		(
			"axum_http_requests_pending",
			["method=\"GET\"", "endpoint=\"/api/workspaces/{id}\""].as_slice(),
			" 0",
		),
	] {
		assert!(
			metrics
				.lines()
				.any(|line| line.starts_with(&format!("{name}{{"))
					&& labels.iter().all(|label| line.contains(label))
					&& line.ends_with(value)),
			"{metrics}"
		);
	}
	let log = app.log();
	let response_logs: Vec<_> = log
		.lines()
		.filter(|line| line.contains("HTTP response"))
		.collect();
	assert!(
		response_logs
			.iter()
			.any(|line| line.contains("method=\"GET\"")
				&& line.contains("route=\"/api/workspaces/{id}\"")),
		"{log}"
	);
	assert!(
		response_logs
			.iter()
			.all(|line| !line.contains("fixture-private-query") && !line.contains(&id.to_string())),
		"{log}"
	);
	assert!(
		metrics
			.lines()
			.any(|line| line == "aidash_sse_reconcile_interval_seconds 2")
	);
	assert!(
		metrics
			.lines()
			.any(|line| line == "aidash_sse_backpressure_timeout_seconds 7")
	);
	app.shutdown().await;
}

#[rstest]
#[tokio::test]
async fn manage_runworker_recovers_durable_work_and_exposes_only_probes(
	#[future] native_worker: ManageFixture,
) {
	use aidash_server::apps::federation::transactions::models::states::AtomicCoordinatorDecision;
	use aidash_server::apps::federation::transactions::models::{AtomicCoordinator, AtomicHistory};
	use chrono::{Duration, Utc};
	// Arrange: recovery must find a durable, undecided transaction without an HTTP wake-up.
	let mut app = native_worker.await;
	let id = Uuid::new_v4();
	let manifest = json!({"id":id,"coordinator":"aidash://manage-fixture","isolation":"serializable",
		"deadline":Utc::now()-Duration::seconds(1),"participants":[{"node_id":"aidash://manage-fixture",
		"mutations":[{"kind":"workspace_state","workspace_id":Uuid::new_v4(),"expected_revision":0,"state":{}}]}]});
	let typed: aidash_server::transactions::Manifest =
		serde_json::from_value(manifest.clone()).unwrap();
	let transaction = AtomicCoordinator::build()
		.id(id)
		.digest(typed.digest().unwrap())
		.manifest(manifest.into())
		.decision(None)
		.visible(false)
		.complete(false)
		.last_error(None)
		.finish();
	AtomicCoordinator::objects()
		.create_with_conn(&mut app.database.lease.handle(), &transaction)
		.await
		.unwrap();
	// Act
	let saved = tokio::time::timeout(std::time::Duration::from_secs(15), async {
		loop {
			if let Some(status) = app.process.try_wait().unwrap() {
				panic!("worker exited {status}: {}", app.log());
			}
			let saved = AtomicCoordinator::objects()
				.filter(AtomicCoordinator::field_id().eq(id))
				.get_with_db(&mut app.database.lease.handle())
				.await
				.unwrap();
			if saved.decision.is_some() {
				break saved;
			}
			tokio::time::sleep(std::time::Duration::from_millis(50)).await;
		}
	})
	.await
	.expect("worker recovers the durable transaction");
	// Assert
	assert_eq!(saved.decision, Some(AtomicCoordinatorDecision::Abort));
	let history = AtomicHistory::objects()
		.filter(AtomicHistory::field_transaction_id().eq(id))
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert!(history.iter().any(|row| row.phase == "ABORT"));
	assert_eq!(app.probes.get("/live").await.unwrap().status_code(), 200);
	assert_eq!(app.probes.get("/ready").await.unwrap().status_code(), 200);
	assert_eq!(
		app.client.get("/api/state").await.unwrap().status_code(),
		404
	);
	assert_eq!(
		app.client
			.get("/.well-known/aidash")
			.await
			.unwrap()
			.status_code(),
		404
	);
	app.shutdown().await;
}
