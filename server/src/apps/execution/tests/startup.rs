use crate::manage::{ManageFixture, native_server, native_worker};
use aidash_server::apps::workspaces::models::Thread;
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
	let store = aidash_server::store::Store::from_pool(
		app.database.connection.clone().into_postgres().unwrap(),
		aidash_domain::nodes::DeploymentId::parse("aidash://manage-fixture").unwrap(),
	)
	.await
	.unwrap();
	let token = crate::tenant_roots::tenant_token(&store, "acme").await;
	app.client
		.set_header("Authorization", &format!("Bearer {token}"))
		.await
		.unwrap();
	// Act
	let response = app
		.client
		.post("/api/threads", &json!({"title":"Native startup"}), "json")
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
	let thread = Thread::objects()
		.filter(Thread::field_id().eq(id))
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	// Assert
	assert_eq!(thread.title, "Native startup");
	assert_eq!(identity["id"], "aidash://manage-fixture");
	assert_eq!(
		app.client
			.get(&format!("/api/threads/{id}?token=fixture-private-query"))
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
				"endpoint=\"/api/threads\"",
				"status=\"200\"",
			]
			.as_slice(),
			" 1",
		),
		(
			"axum_http_requests_total",
			[
				"method=\"GET\"",
				"endpoint=\"/api/threads/{id}\"",
				"status=\"200\"",
			]
			.as_slice(),
			" 1",
		),
		(
			"axum_http_requests_pending",
			["method=\"GET\"", "endpoint=\"/api/threads/{id}\""].as_slice(),
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
				&& line.contains("route=\"/api/threads/{id}\"")),
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
	let node =
		aidash_domain::nodes::NodeAddress::derived("aidash://manage-fixture", "manage").to_string();
	let manifest = json!({"id":id,"coordinator":node,"isolation":"serializable",
		"deadline":Utc::now()-Duration::seconds(1),"participants":[{"node_id":node,
		"mutations":[{"kind":"thread_title","thread_id":Uuid::new_v4(),"expected_revision":0,"title":"Committed"}]}]});
	let typed: aidash_server::transactions::Manifest =
		serde_json::from_value(manifest.clone()).unwrap();
	let transaction = AtomicCoordinator::build()
		.id(id)
		.digest(typed.sealed_digest(&"5e".repeat(32)).unwrap())
		.manifest(manifest.into())
		.seed("5e".repeat(32))
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

#[test]
fn an_egress_gateway_on_a_secret_manager_store_needs_the_tenant_secret_broker() {
	// Arrange: the Secret Manager Store seals hosts with the value, so the
	// gateway cannot read a Tenant Secret and must forward to the broker.
	// Act
	let error = aidash_server::bootstrap::require_egress_broker_settings(true, false)
		.unwrap_err()
		.to_string();
	// Assert: the refusal names the missing settings, not a vague capability.
	assert!(error.contains("tenant_secret_broker"), "{error}");
	assert!(error.contains("endpoint"), "{error}");
	assert!(aidash_server::bootstrap::require_egress_broker_settings(true, true).is_ok());
	// A PostgreSQL Store reads the value itself, so it stays direct.
	assert!(aidash_server::bootstrap::require_egress_broker_settings(false, false).is_ok());
}
