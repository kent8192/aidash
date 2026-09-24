mod common;

use aidash::{
	api,
	domain::{NewTask, qualified_agent},
	federation::{Federation, Home},
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
			"run_message_output"
				| "run_message_history"
				| "run_message_delivery"
				| "run_message_delivery_capability"
				| "run_message_reserve"
				| "run_message_commit"
				| "run_message_release"
				| "run_message_ack"
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
	let home_db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(home.store.pool.clone());
	// Seed a pre-ledger correction, then make the executor admission hit the
	// old-worker lease fence. Historical recovery must keep the home reservation.
	Migrator::down(&home_db, Some(3)).await.unwrap();
	let first_key = Uuid::new_v4();
	home.store
		.message(
			workspace.id,
			&format!("human@{}", executor.config.node_id),
			"remote correction",
			Some(&format!(
				"{}:{}:human:{}:{first_key}",
				executor.config.node_id, task.id, run.id
			)),
		)
		.await
		.unwrap();
	Migrator::up(&home_db, None).await.unwrap();
	let mut old_lease = executor.store.pool.begin().await.unwrap();
	sqlx::query_scalar::<_, String>(
		&Query::select()
			.expr(Expr::cust(
				"set_config('aidash.input_ledger_worker', 'true', true)",
			))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *old_lease)
	.await
	.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("runs"))
			.value(Alias::new("lease_owner"), Expr::cust("$2"))
			.value(
				Alias::new("lease_until"),
				Expr::cust("CURRENT_TIMESTAMP + INTERVAL '10 minutes'"),
			)
			.and_where(Expr::cust("id = $1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(Uuid::new_v4())
	.execute(&mut *old_lease)
	.await
	.unwrap();
	old_lease.commit().await.unwrap();
	let observation = executor_app
		.clone()
		.oneshot(
			Request::get("/federation/v0.1/observe")
				.header(
					"authorization",
					format!(
						"Bearer {}",
						std::env::var("AIDASH_SECRET_TEST_PEER").unwrap()
					),
				)
				.header("x-aidash-node", &home.config.node_id)
				.header("x-aidash-protocol", "0.1")
				.body(Body::empty())
				.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(
		observation.status(),
		200,
		"peer observations must decode the full Run"
	);
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
	let first_input_key = format!("human:{}:{first_key}", run.id);
	let remote_home = Home::new(executor.clone(), run.clone());
	let legacy_output_key = format!("{}:{}:output", run.id, run.step);
	assert!(
		remote_home
			.message(&legacy_output_key, "stale legacy remote response")
			.await
			.is_err()
	);
	assert!(
		home.store
			.snapshot(workspace.id)
			.await
			.unwrap()
			.messages
			.iter()
			.all(|message| message.content != "stale legacy remote response")
	);
	assert!(
		remote_home
			.reserve_run_message(&first_input_key, "changed correction")
			.await
			.is_err(),
		"a reservation cannot be reused with different content"
	);
	assert!(
		remote_home
			.human_message_record(&first_input_key, "changed correction")
			.await
			.is_err(),
		"delivery must match the reserved content"
	);
	let mut local_run = run.clone();
	local_run.home_node = executor.config.node_id.clone();
	let local_home = Home::new(executor.clone(), local_run);
	assert!(
		local_home
			.reserve_run_message(&first_input_key, "local reservation")
			.await
			.unwrap()
	);
	local_home.release_run_messages(&[]).await.unwrap();
	local_home.acknowledge_run_messages(&[]).await.unwrap();
	let lease_key = format!("human:{}:{}", run.id, Uuid::new_v4());
	home.store
		.reserve_remote_run_message(
			task.id,
			run.id,
			&executor.config.node_id,
			&lease_key,
			"lease renewal",
		)
		.await
		.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("remote_run_message_fences"))
			.value(
				Alias::new("expires_at"),
				Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 second'"),
			)
			.and_where(Expr::cust(
				"task_id = $1 AND run_id = $2 AND idempotency_key = $3",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(task.id)
	.bind(run.id)
	.bind(&lease_key)
	.execute(&home.store.pool)
	.await
	.unwrap();
	assert!(
		remote_home
			.human_message_record(&lease_key, "lease renewal")
			.await
			.is_err(),
		"delivery cannot revive an expired reservation by itself"
	);
	assert!(
		remote_home
			.reserve_run_message(&lease_key, "lease renewal")
			.await
			.unwrap()
	);
	assert_eq!(
		remote_home
			.human_message_record(&lease_key, "lease renewal")
			.await
			.unwrap()
			.content,
		"lease renewal"
	);
	remote_home
		.acknowledge_run_messages(std::slice::from_ref(&lease_key))
		.await
		.unwrap();
	home.store
		.acknowledge_remote_run_messages(task.id, run.id, &[])
		.await
		.unwrap();
	remote_home
		.release_run_messages(std::slice::from_ref(&lease_key))
		.await
		.unwrap();
	let retained_consumed_fence: Option<bool> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("consumed"))
			.from(Alias::new("remote_run_message_fences"))
			.and_where(Expr::cust(
				"task_id = $1 AND run_id = $2 AND idempotency_key = $3",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(task.id)
	.bind(run.id)
	.bind(&lease_key)
	.fetch_optional(&home.store.pool)
	.await
	.unwrap();
	assert_eq!(retained_consumed_fence, Some(true));
	assert!(
		home.store
			.reserve_remote_run_message(
				task.id,
				Uuid::new_v4(),
				&executor.config.node_id,
				&first_input_key,
				"remote correction",
			)
			.await
			.is_err(),
		"an idempotency key cannot be reused by another run"
	);
	let released_key = format!("human:{}:{}", run.id, Uuid::new_v4());
	remote_home
		.reserve_run_message(&released_key, "rejected admission")
		.await
		.unwrap();
	remote_home
		.release_run_messages(std::slice::from_ref(&released_key))
		.await
		.unwrap();
	let released_count: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("remote_run_message_fences"))
			.and_where(Expr::cust(
				"task_id = $1 AND run_id = $2 AND idempotency_key = $3",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(task.id)
	.bind(run.id)
	.bind(&released_key)
	.fetch_one(&home.store.pool)
	.await
	.unwrap();
	assert_eq!(released_count, 0);
	let mut terminal_history_run = run.clone();
	terminal_history_run.id = Uuid::new_v4();
	let terminal_history_key = format!("human:{}:{}", terminal_history_run.id, Uuid::new_v4());
	let terminal_history_content = "terminal historical correction";
	home.store
		.reserve_remote_run_message(
			task.id,
			terminal_history_run.id,
			&executor.config.node_id,
			&terminal_history_key,
			terminal_history_content,
		)
		.await
		.unwrap();
	let terminal_history_home = Home::new(executor.clone(), terminal_history_run.clone());
	terminal_history_home
		.human_message_record(&terminal_history_key, terminal_history_content)
		.await
		.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("remote_run_message_fences"))
			.value(
				Alias::new("expires_at"),
				Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 second'"),
			)
			.and_where(Expr::cust(
				"task_id = $1 AND run_id = $2 AND idempotency_key = $3",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(task.id)
	.bind(terminal_history_run.id)
	.bind(&terminal_history_key)
	.execute(&home.store.pool)
	.await
	.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("runs"))
			.value(Alias::new("lease_owner"), Expr::cust("NULL"))
			.value(Alias::new("lease_until"), Expr::cust("NULL"))
			.and_where(Expr::cust("id = $1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.execute(&executor.store.pool)
	.await
	.unwrap();
	let unreserved_key = format!("human:{}:{}", run.id, Uuid::new_v4());
	assert!(
		Home::new(executor.clone(), run.clone())
			.human_message_record(&unreserved_key, "unreserved injection")
			.await
			.is_err(),
		"terminal-safe delivery requires a matching reservation"
	);
	let response_worker = Uuid::new_v4();
	let response_run = executor
		.store
		.lease_run(response_worker, 30)
		.await
		.unwrap()
		.unwrap();
	let included_input_seq = executor
		.store
		.run_inputs(run.id)
		.await
		.unwrap()
		.last()
		.unwrap()
		.seq;
	assert!(matches!(
		Home::new(executor.clone(), response_run)
			.response_message(
				response_worker,
				included_input_seq,
				&format!("{}:response-output", run.id),
				"response must wait until the correction is observed",
			)
			.await,
		Err(aidash::error::Error::TransactionPending)
	));
	assert!(
		!home
			.store
			.snapshot(workspace.id)
			.await
			.unwrap()
			.messages
			.iter()
			.any(|message| message.content == "response must wait until the correction is observed")
	);
	executor
		.store
		.release_lease(run.id, response_worker)
		.await
		.unwrap();
	assert!(
		!home
			.store
			.snapshot(workspace.id)
			.await
			.unwrap()
			.messages
			.iter()
			.any(|message| message.content == "unreserved injection")
	);
	let current_task = home.store.task(task.id).await.unwrap();
	let error = home
		.store
		.transition(task.id, current_task.revision, &owner, "CANCELLED")
		.await
		.expect_err("home termination must wait until the correction reaches inference");
	let response = error.into_response();
	assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
	assert_eq!(
		response
			.headers()
			.get("x-aidash-run-message-pending")
			.and_then(|value| value.to_str().ok()),
		Some("1")
	);
	assert!(matches!(
		Home::new(executor.clone(), run.clone())
			.transition("CANCELLED")
			.await,
		Err(aidash::error::Error::TransactionPending)
	));
	assert!(
		home.store
			.snapshot(workspace.id)
			.await
			.unwrap()
			.messages
			.iter()
			.any(|message| message.content == "remote correction")
	);
	remote_home
		.acknowledge_run_messages(std::slice::from_ref(&first_input_key))
		.await
		.unwrap();
	let response_worker = Uuid::new_v4();
	let response_run = executor
		.store
		.lease_run(response_worker, 30)
		.await
		.unwrap()
		.unwrap();
	let included_input_seq = executor
		.store
		.run_inputs(run.id)
		.await
		.unwrap()
		.last()
		.unwrap()
		.seq;
	Home::new(executor.clone(), response_run)
		.response_message(
			response_worker,
			included_input_seq,
			&format!("{}:observed-response-output", run.id),
			"response output after correction observation",
		)
		.await
		.unwrap();
	assert!(
		home.store
			.snapshot(workspace.id)
			.await
			.unwrap()
			.messages
			.iter()
			.any(|message| message.content == "response output after correction observation")
	);
	executor
		.store
		.release_lease(run.id, response_worker)
		.await
		.unwrap();
	// Historical writes were possible before the new home database gate.
	Migrator::down(&home_db, Some(3)).await.unwrap();
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
	let legacy_message_key = format!(
		"{}:{}:human:{}:{legacy_key}",
		executor.config.node_id, task.id, run.id
	);
	let legacy_message = home
		.store
		.snapshot(workspace.id)
		.await
		.unwrap()
		.messages
		.into_iter()
		.find(|message| message.idempotency_key.as_deref() == Some(legacy_message_key.as_str()))
		.unwrap();
	let legacy_input_key = format!("human:{}:{legacy_key}", run.id);
	let input_limit = executor.run_message_limit(&run).await.unwrap();
	executor
		.store
		.import_remote_run_message(run.id, &legacy_input_key, &legacy_message, input_limit)
		.await
		.unwrap();
	executor
		.store
		.import_remote_run_messages(run.id, &[], input_limit)
		.await
		.unwrap();
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
	let ordered_sequences: Vec<i64> = (0..5)
		.map(|index| {
			inputs
				.iter()
				.find(|input| input.content == format!("paged historical correction {index}"))
				.unwrap()
				.seq
		})
		.collect();
	assert!(
		ordered_sequences.windows(2).all(|pair| pair[0] < pair[1]),
		"one history batch retains the home's message order"
	);
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
	let mut old_peer_observed = run.clone();
	old_peer_observed.observed_input_seq = executor
		.store
		.run_inputs(run.id)
		.await
		.unwrap()
		.last()
		.unwrap()
		.seq;
	executor
		.acknowledge_run_messages(&old_peer_observed)
		.await
		.unwrap();
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
	let admitted_fence_is_durable: bool = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust(
				"EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = $1 AND run_id = $2 AND idempotency_key = $3 AND expires_at IS NULL AND NOT consumed)",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(task.id)
	.bind(run.id)
	.bind(format!("human:{}:{pending_key}", run.id))
	.fetch_one(&home.store.pool)
	.await
	.unwrap();
	assert!(
		admitted_fence_is_durable,
		"successful admission stores a non-expiring fence even while delivery is unavailable"
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
	assert!(
		home.store
			.transition(task.id, current_task.revision, &owner, "CANCELLED")
			.await
			.is_err(),
		"admitted input must fence cancellation even during delivery outage"
	);
	// Model the successful inference acknowledgment that precedes task exit.
	let mut observed = executor.store.run(run.id).await.unwrap();
	observed.observed_input_seq = executor
		.store
		.run_inputs(run.id)
		.await
		.unwrap()
		.last()
		.unwrap()
		.seq;
	executor.acknowledge_run_messages(&observed).await.unwrap();
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
	let (status, body) = common::request(
		&executor_app,
		&executor.config.api_token,
		"POST",
		&format!("/api/runs/{}/message", run.id),
		json!({"content":"direct message after home cancellation","idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 409, "{body}");
	assert!(
		!executor
			.store
			.run_inputs(run.id)
			.await
			.unwrap()
			.iter()
			.any(|input| input.content == "direct message after home cancellation")
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
	let (status, body) = common::request(
		&executor_app,
		&executor.config.api_token,
		"POST",
		&format!("/api/runs/{}/message", run.id),
		json!({"content":"remote correction","idempotency_key":first_key}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
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
	let expiry_task = home
		.store
		.create_task(
			workspace.id,
			&NewTask {
				title: "Expired reservation".into(),
				description: "A stale API reservation must not block cancellation".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"human",
			None,
		)
		.await
		.unwrap();
	let expired_run = Uuid::new_v4();
	let expired_key = format!("human:{expired_run}:{}", Uuid::new_v4());
	home.store
		.reserve_remote_run_message(
			expiry_task.id,
			expired_run,
			&executor.config.node_id,
			&expired_key,
			"stale reservation",
		)
		.await
		.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("remote_run_message_fences"))
			.value(
				Alias::new("expires_at"),
				Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 second'"),
			)
			.and_where(Expr::cust("task_id = $1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(expiry_task.id)
	.execute(&home.store.pool)
	.await
	.unwrap();
	assert!(
		home.store
			.commit_remote_run_message(
				expiry_task.id,
				expired_run,
				&expired_key,
				"stale reservation"
			)
			.await
			.is_err(),
		"an expired orphan reservation cannot be promoted to an admitted input"
	);
	let cancelled = home
		.store
		.transition(expiry_task.id, expiry_task.revision, "human", "CANCELLED")
		.await
		.unwrap();
	assert_eq!(cancelled.status, "CANCELLED");
	home.store
		.reserve_remote_run_message(
			task.id,
			terminal_history_run.id,
			&executor.config.node_id,
			&terminal_history_key,
			terminal_history_content,
		)
		.await
		.unwrap();
	let historical_reservation_committed: bool = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust(
				"EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = $1 AND run_id = $2 AND idempotency_key = $3 AND expires_at IS NULL)",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(task.id)
	.bind(terminal_history_run.id)
	.bind(&terminal_history_key)
	.fetch_one(&home.store.pool)
	.await
	.unwrap();
	assert!(historical_reservation_committed);
	assert_eq!(
		terminal_history_home
			.human_message_record(&terminal_history_key, terminal_history_content)
			.await
			.unwrap()
			.content,
		terminal_history_content
	);
	let absent_terminal_key = format!("human:{}:{}", terminal_history_run.id, Uuid::new_v4());
	assert!(
		home.store
			.reserve_remote_run_message(
				task.id,
				terminal_history_run.id,
				&executor.config.node_id,
				&absent_terminal_key,
				"not in home history",
			)
			.await
			.is_err(),
		"terminal tasks accept only exact historical message keys"
	);
	terminal_history_home
		.acknowledge_run_messages(std::slice::from_ref(&terminal_history_key))
		.await
		.unwrap();
	terminal_history_home
		.release_run_messages(std::slice::from_ref(&terminal_history_key))
		.await
		.unwrap();
	let consumed_terminal_reservation: bool = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("consumed"))
			.from(Alias::new("remote_run_message_fences"))
			.and_where(Expr::cust(
				"task_id = $1 AND run_id = $2 AND idempotency_key = $3",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(task.id)
	.bind(terminal_history_run.id)
	.bind(&terminal_history_key)
	.fetch_one(&home.store.pool)
	.await
	.unwrap();
	assert!(consumed_terminal_reservation);
	let mut invalid_terminal_run = run.clone();
	invalid_terminal_run.id = Uuid::new_v4();
	let invalid_terminal_key = format!("human:{}:{}", invalid_terminal_run.id, Uuid::new_v4());
	sqlx::query("ALTER TABLE tasks DISABLE TRIGGER gate_remote_task_terminal")
		.execute(&home.store.pool)
		.await
		.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("tasks"))
			.value(Alias::new("status"), Expr::cust("'RUNNING'"))
			.and_where(Expr::cust("id = $1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(task.id)
	.execute(&home.store.pool)
	.await
	.unwrap();
	sqlx::query("ALTER TABLE tasks ENABLE TRIGGER gate_remote_task_terminal")
		.execute(&home.store.pool)
		.await
		.unwrap();
	home.store
		.reserve_remote_run_message(
			task.id,
			invalid_terminal_run.id,
			&executor.config.node_id,
			&invalid_terminal_key,
			"uncommitted terminal delivery",
		)
		.await
		.unwrap();
	sqlx::query("ALTER TABLE tasks DISABLE TRIGGER gate_remote_task_terminal")
		.execute(&home.store.pool)
		.await
		.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("tasks"))
			.value(Alias::new("status"), Expr::cust("'CANCELLED'"))
			.and_where(Expr::cust("id = $1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(task.id)
	.execute(&home.store.pool)
	.await
	.unwrap();
	sqlx::query("ALTER TABLE tasks ENABLE TRIGGER gate_remote_task_terminal")
		.execute(&home.store.pool)
		.await
		.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("remote_run_message_fences"))
			.value(Alias::new("consumed"), Expr::cust("FALSE"))
			.value(
				Alias::new("expires_at"),
				Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 second'"),
			)
			.and_where(Expr::cust(
				"task_id = $1 AND run_id = $2 AND idempotency_key = $3",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(task.id)
	.bind(terminal_history_run.id)
	.bind(&terminal_history_key)
	.execute(&home.store.pool)
	.await
	.unwrap();
	home.store
		.reserve_remote_run_message(
			task.id,
			terminal_history_run.id,
			&executor.config.node_id,
			&terminal_history_key,
			terminal_history_content,
		)
		.await
		.unwrap();
	let recovered_terminal_reservation: bool = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust(
				"EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = $1 AND run_id = $2 AND idempotency_key = $3 AND expires_at IS NULL AND NOT consumed)",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(task.id)
	.bind(terminal_history_run.id)
	.bind(&terminal_history_key)
	.fetch_one(&home.store.pool)
	.await
	.unwrap();
	assert!(recovered_terminal_reservation);
	terminal_history_home
		.human_message_record(&terminal_history_key, terminal_history_content)
		.await
		.unwrap();
	assert!(
		Home::new(executor.clone(), invalid_terminal_run)
			.human_message_record(&invalid_terminal_key, "uncommitted terminal delivery")
			.await
			.is_err(),
		"terminal delivery cannot commit an unconsumed reservation"
	);
	server.abort();
	cleanup(home, &home_url, &home_schema).await;
	cleanup(executor, &executor_url, &executor_schema).await;
}
