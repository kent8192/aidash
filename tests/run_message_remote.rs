mod common;

use aidash::{
	api,
	domain::{NewTask, qualified_agent},
	federation::Federation,
	harness::Harness,
};
use axum::{Router, body::Body, http::Request, middleware::Next, response::IntoResponse};
use common::{bootstrap, cleanup, setup};
use migration::{Migrator, MigratorTrait};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicBool, Ordering},
};
use tower::ServiceExt;
use uuid::Uuid;

async fn peer_control(app: &Router, node: &str, body: Value) -> (u16, Value) {
	let secret = std::env::var("AIDASH_SECRET_TEST_PEER").unwrap();
	let response = app
		.clone()
		.oneshot(
			Request::post("/federation/v0.1/control")
				.header("authorization", format!("Bearer {secret}"))
				.header("x-aidash-node", node)
				.header("x-aidash-protocol", "0.1")
				.header("content-type", "application/json")
				.body(Body::from(body.to_string()))
				.unwrap(),
		)
		.await
		.unwrap();
	let status = response.status().as_u16();
	let bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
		.await
		.unwrap();
	(
		status,
		serde_json::from_slice(&bytes).unwrap_or(Value::Null),
	)
}

async fn add_peer(f: &Federation, node: &str, endpoint: &str) {
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("peers"))
			.columns([
				Alias::new("node_id"),
				Alias::new("endpoint"),
				Alias::new("credential_env"),
				Alias::new("protocol_version"),
				Alias::new("enabled"),
			])
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::cust("'AIDASH_SECRET_TEST_PEER'"),
				Expr::cust("'0.1'"),
				Expr::cust("TRUE"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(node)
	.bind(endpoint)
	.execute(&f.store.pool)
	.await
	.unwrap();
}

struct PeerMode {
	old_peer: AtomicBool,
	delivery_outage: AtomicBool,
}

async fn old_peer_workspace_compat(
	axum::extract::State(mode): axum::extract::State<Arc<PeerMode>>,
	request: Request<Body>,
	next: Next,
) -> axum::response::Response {
	if !request.uri().path().ends_with("/workspace")
		|| (!mode.old_peer.load(Ordering::SeqCst) && !mode.delivery_outage.load(Ordering::SeqCst))
	{
		return next.run(request).await;
	}
	let (parts, body) = request.into_parts();
	let bytes = axum::body::to_bytes(body, 1_048_576).await.unwrap();
	let command: Value = serde_json::from_slice(&bytes).unwrap();
	let operation = command["operation"].as_str().unwrap();
	if mode.delivery_outage.load(Ordering::SeqCst) && operation == "run_message_delivery" {
		return (
			axum::http::StatusCode::SERVICE_UNAVAILABLE,
			axum::Json(json!({"error":"simulated delivery outage"})),
		)
			.into_response();
	}
	if mode.old_peer.load(Ordering::SeqCst)
		&& matches!(
			operation,
			"run_message_history" | "run_message_delivery" | "run_message_delivery_capability"
		) {
		return (
			axum::http::StatusCode::BAD_REQUEST,
			axum::Json(json!({"error":"unknown federation operation"})),
		)
			.into_response();
	}
	let response = next
		.run(Request::from_parts(parts, Body::from(bytes)))
		.await;
	if mode.old_peer.load(Ordering::SeqCst)
		&& operation == "human_message"
		&& response.status().is_success()
	{
		return axum::Json(json!({"sent":true})).into_response();
	}
	response
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn remote_control_admits_before_delivery_and_rejects_late_side_effects() {
	let (mut home, home_url, home_schema) = setup().await;
	let (mut executor, executor_url, executor_schema) = setup().await;
	executor.config.node_id = "aidash://run-message-executor".into();
	executor.store.node_id = executor.config.node_id.clone();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	home.config.endpoint = format!("http://{}", listener.local_addr().unwrap());
	let mode = Arc::new(PeerMode {
		old_peer: AtomicBool::new(false),
		delivery_outage: AtomicBool::new(false),
	});
	let home_app = api::router(home.clone()).layer(axum::middleware::from_fn_with_state(
		mode.clone(),
		old_peer_workspace_compat,
	));
	let executor_app = api::router(executor.clone());
	bootstrap(&home, &home_app, "http://127.0.0.1:9").await;
	bootstrap(&executor, &executor_app, "http://127.0.0.1:9").await;
	add_peer(&home, &executor.config.node_id, "http://127.0.0.1:9").await;
	add_peer(&executor, &home.config.node_id, &home.config.endpoint).await;
	let server = tokio::spawn(async move { axum::serve(listener, home_app).await.unwrap() });
	let workspace = home
		.store
		.create_workspace("Remote corrections", "Complete work")
		.await
		.unwrap();
	let task = home
		.store
		.create_task(
			workspace.id,
			&NewTask {
				title: "Answer".into(),
				description: "Reply".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"human",
			None,
		)
		.await
		.unwrap();
	let agent = home.registry.get("research", "1.0.0").await.unwrap();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("delegations"))
			.columns([
				Alias::new("task_id"),
				Alias::new("node_id"),
				Alias::new("agent_id"),
				Alias::new("agent_version"),
			])
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::cust("$3"),
				Expr::cust("$4"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(task.id)
	.bind(&executor.config.node_id)
	.bind(&agent.id)
	.bind(&agent.version)
	.execute(&home.store.pool)
	.await
	.unwrap();
	let owner = qualified_agent(&executor.config.node_id, &agent.id, &agent.version);
	let task = home
		.store
		.claim(task.id, task.revision, &owner, &agent)
		.await
		.unwrap();
	let task = home
		.store
		.transition(task.id, task.revision, &owner, "RUNNING")
		.await
		.unwrap();
	let run = executor
		.store
		.accept_run(&task, &home.config.node_id, &agent.id, &agent.version)
		.await
		.unwrap();
	let first_key = Uuid::new_v4();
	let (status, body) = peer_control(
		&executor_app,
		&home.config.node_id,
		json!({
			"run_id":run.id,"action":"message","content":"remote correction","idempotency_key":first_key
		}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let inputs = executor.store.run_inputs(run.id).await.unwrap();
	assert_eq!(inputs.len(), 1);
	assert!(inputs[0].message_id.is_some());
	assert!(
		home.store
			.snapshot(workspace.id)
			.await
			.unwrap()
			.messages
			.iter()
			.any(|message| message.content == "remote correction")
	);
	let home_db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(home.store.pool.clone());
	// Historical writes were possible before the new home database gate.
	Migrator::down(&home_db, Some(1)).await.unwrap();
	let legacy_key = Uuid::new_v4();
	home.store
		.message(
			workspace.id,
			&format!("human@{}", executor.config.node_id),
			"before upgrade",
			Some(&format!(
				"{}:{}:human:{}:{legacy_key}",
				executor.config.node_id, task.id, run.id
			)),
		)
		.await
		.unwrap();
	executor.reconcile_run_messages(&run).await.unwrap();
	assert!(
		executor
			.store
			.run_inputs(run.id)
			.await
			.unwrap()
			.iter()
			.any(|input| input.content == "before upgrade")
	);
	let large_key = Uuid::new_v4();
	let large_content = "historic correction ".repeat(1500);
	home.store
		.message(
			workspace.id,
			&format!("human@{}", executor.config.node_id),
			&large_content,
			Some(&format!(
				"{}:{}:human:{}:{large_key}",
				executor.config.node_id, task.id, run.id
			)),
		)
		.await
		.unwrap();
	executor.reconcile_run_messages(&run).await.unwrap();
	assert!(
		executor
			.store
			.run_inputs(run.id)
			.await
			.unwrap()
			.iter()
			.any(|input| input.content == large_content && input.reference_only)
	);
	for index in 0..5 {
		home.store
			.message(
				workspace.id,
				&format!("human@{}", executor.config.node_id),
				&format!("paged historical correction {index}"),
				Some(&format!(
					"{}:{}:human:{}:{}",
					executor.config.node_id,
					task.id,
					run.id,
					Uuid::new_v4()
				)),
			)
			.await
			.unwrap();
	}
	executor.reconcile_run_messages(&run).await.unwrap();
	let inputs = executor.store.run_inputs(run.id).await.unwrap();
	for index in 0..5 {
		assert!(
			inputs
				.iter()
				.any(|input| input.content == format!("paged historical correction {index}"))
		);
	}
	mode.old_peer.store(true, Ordering::SeqCst);
	let old_home_key = Uuid::new_v4();
	home.store
		.message(
			workspace.id,
			&format!("human@{}", executor.config.node_id),
			"old-home historical correction",
			Some(&format!(
				"{}:{}:human:{}:{old_home_key}",
				executor.config.node_id, task.id, run.id
			)),
		)
		.await
		.unwrap();
	executor.reconcile_run_messages(&run).await.unwrap();
	assert!(
		executor
			.store
			.run_inputs(run.id)
			.await
			.unwrap()
			.iter()
			.any(|input| input.content == "old-home historical correction")
	);
	let compatibility_key = Uuid::new_v4();
	assert_eq!(
		peer_control(
			&executor_app,
			&home.config.node_id,
			json!({
				"run_id":run.id,"action":"message","content":"old peer correction","idempotency_key":compatibility_key
			})
		)
		.await
		.0,
		409
	);
	assert!(
		!home
			.store
			.snapshot(workspace.id)
			.await
			.unwrap()
			.messages
			.iter()
			.any(|message| message.content == "old peer correction")
	);
	// Inputs accepted before capability gating still use the legacy active-task
	// delivery fallback while that peer remains on the preceding release.
	executor
		.store
		.accept_run_message(
			run.id,
			"human",
			"old peer correction",
			&format!("human:{}:{compatibility_key}", run.id),
			executor.run_message_limit(&run).await.unwrap(),
		)
		.await
		.unwrap();
	executor.deliver_run_messages(&run).await.unwrap();
	assert!(
		executor
			.store
			.run_inputs(run.id)
			.await
			.unwrap()
			.iter()
			.any(|input| input.content == "old peer correction" && input.message_id.is_some())
	);
	mode.old_peer.store(false, Ordering::SeqCst);
	Migrator::up(&home_db, None).await.unwrap();
	// The rollout gate allows a previously committed old-peer message to be
	// retried exactly, while it continues to reject new legacy admissions.
	let old_home_message_key = format!(
		"{}:{}:human:{}:{old_home_key}",
		executor.config.node_id, task.id, run.id
	);
	home.store
		.message(
			workspace.id,
			&format!("human@{}", executor.config.node_id),
			"old-home historical correction",
			Some(&old_home_message_key),
		)
		.await
		.unwrap();
	assert!(
		home.store
			.message(
				workspace.id,
				&format!("human@{}", executor.config.node_id),
				"changed historical correction",
				Some(&old_home_message_key),
			)
			.await
			.is_err()
	);
	mode.delivery_outage.store(true, Ordering::SeqCst);
	let pending_key = Uuid::new_v4();
	assert_eq!(
		peer_control(
			&executor_app,
			&home.config.node_id,
			json!({
				"run_id":run.id,"action":"message","content":"queued delivery","idempotency_key":pending_key
			})
		)
		.await
		.0,
		200
	);
	assert!(
		executor
			.store
			.run_inputs(run.id)
			.await
			.unwrap()
			.iter()
			.any(|input| input.content == "queued delivery" && input.message_id.is_none())
	);
	assert!(
		!home
			.store
			.snapshot(workspace.id)
			.await
			.unwrap()
			.messages
			.iter()
			.any(|message| message.content == "queued delivery")
	);
	mode.delivery_outage.store(false, Ordering::SeqCst);
	let current_task = home.store.task(task.id).await.unwrap();
	home.store
		.transition(task.id, current_task.revision, &owner, "CANCELLED")
		.await
		.unwrap();
	let rejected_after_home_terminal = Uuid::new_v4();
	assert_eq!(
		peer_control(
			&executor_app,
			&home.config.node_id,
			json!({
				"run_id":run.id,"action":"message","content":"after home cancellation","idempotency_key":rejected_after_home_terminal
			})
		)
		.await
		.0,
		409,
		"the executor must consult the home task before admitting a new message"
	);
	assert!(
		!executor
			.store
			.run_inputs(run.id)
			.await
			.unwrap()
			.iter()
			.any(|input| input.content == "after home cancellation")
	);
	sqlx::query(
		&Query::update()
			.table(Alias::new("runs"))
			.value(Alias::new("phase"), Expr::cust("'CANCELLED'"))
			.and_where(Expr::cust("id = $1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.execute(&executor.store.pool)
	.await
	.unwrap();
	assert!(
		Harness {
			federation: executor.clone()
		}
		.worker_once()
		.await
		.unwrap()
	);
	assert!(
		executor
			.store
			.run_inputs(run.id)
			.await
			.unwrap()
			.iter()
			.any(|input| input.content == "queued delivery" && input.message_id.is_some())
	);
	sqlx::query(
		&Query::update()
			.table(Alias::new("runs"))
			.value(
				Alias::new("pending"),
				Expr::cust("'{\"finalizing\":true}'::jsonb"),
			)
			.and_where(Expr::cust("id = $1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.execute(&executor.store.pool)
	.await
	.unwrap();
	let late_historical_key = Uuid::new_v4();
	assert!(
		home.store
			.message(
				workspace.id,
				&format!("human@{}", executor.config.node_id),
				"legacy message after finalization",
				Some(&format!(
					"{}:{}:human:{}:{late_historical_key}",
					executor.config.node_id, task.id, run.id
				)),
			)
			.await
			.is_err()
	);
	assert_eq!(
		peer_control(
			&executor_app,
			&home.config.node_id,
			json!({
				"run_id":run.id,"action":"message","content":"legacy message after finalization","idempotency_key":late_historical_key
			})
		)
		.await
		.0,
		409
	);
	assert!(
		!executor
			.store
			.run_inputs(run.id)
			.await
			.unwrap()
			.iter()
			.any(|input| input.content == "legacy message after finalization")
	);
	let (status, body) = peer_control(
		&executor_app,
		&home.config.node_id,
		json!({
			"run_id":run.id,"action":"message","content":"too late","idempotency_key":Uuid::new_v4()
		}),
	)
	.await;
	assert_eq!(status, 409, "{body}");
	assert!(
		!home
			.store
			.snapshot(workspace.id)
			.await
			.unwrap()
			.messages
			.iter()
			.any(|message| message.content == "too late")
	);
	assert_eq!(
		peer_control(
			&executor_app,
			&home.config.node_id,
			json!({
				"run_id":run.id,"action":"message","content":"remote correction","idempotency_key":first_key
			})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		peer_control(
			&executor_app,
			&home.config.node_id,
			json!({
				"run_id":run.id,"action":"message","content":"before upgrade","idempotency_key":legacy_key
			})
		)
		.await
		.0,
		200
	);
	server.abort();
	cleanup(home, &home_url, &home_schema).await;
	cleanup(executor, &executor_url, &executor_schema).await;
}
