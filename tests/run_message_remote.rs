mod common;

use aidash::{
	api,
	domain::{NewTask, qualified_agent},
	federation::Federation,
};
use axum::{Router, body::Body, http::Request};
use common::{bootstrap, cleanup, setup};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
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

async fn set_peer_endpoint(f: &Federation, node: &str, endpoint: &str) {
	sqlx::query(
		&Query::update()
			.table(Alias::new("peers"))
			.value(Alias::new("endpoint"), Expr::cust("$2"))
			.and_where(Expr::cust("node_id = $1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(node)
	.bind(endpoint)
	.execute(&f.store.pool)
	.await
	.unwrap();
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
	let home_app = api::router(home.clone());
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
	set_peer_endpoint(&executor, &home.config.node_id, "http://127.0.0.1:9").await;
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
	set_peer_endpoint(&executor, &home.config.node_id, &home.config.endpoint).await;
	executor.deliver_run_messages(&run).await.unwrap();
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
