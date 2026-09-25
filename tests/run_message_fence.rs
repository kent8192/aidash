mod common;

use aidash::{
	api,
	domain::{Message, NewTask, Run, qualified_agent},
	federation::Federation,
	harness::Harness,
};
use axum::{
	Json, Router,
	body::Body,
	http::{Request, StatusCode},
	middleware::Next,
	response::IntoResponse,
	routing::post,
};
use common::{TestEnvironment, bootstrap, cleanup, request, setup, test_environment};
use migration::{Migrator, MigratorTrait};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;

fn uuid_expr(id: Uuid) -> sea_orm::sea_query::SimpleExpr {
	Expr::cust(format!("'{id}'::uuid"))
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
				Expr::value(node),
				Expr::value(endpoint),
				Expr::value("AIDASH_SECRET_TEST_PEER"),
				Expr::value("0.1"),
				Expr::value(true),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(&f.store.pool)
	.await
	.unwrap();
}

async fn promotion_outage(
	axum::extract::State(outage): axum::extract::State<Arc<AtomicBool>>,
	request: Request<Body>,
	next: Next,
) -> axum::response::Response {
	if !outage.load(Ordering::SeqCst) || !request.uri().path().ends_with("/workspace") {
		return next.run(request).await;
	}
	let (parts, body) = request.into_parts();
	let bytes = axum::body::to_bytes(body, 1_048_576).await.unwrap();
	let command: Value = serde_json::from_slice(&bytes).unwrap();
	if matches!(
		command["operation"].as_str(),
		Some("run_message_commit" | "run_message_release")
	) {
		return (
			StatusCode::SERVICE_UNAVAILABLE,
			Json(json!({"error":"promotion acknowledgement unavailable"})),
		)
			.into_response();
	}
	next.run(Request::from_parts(parts, Body::from(bytes)))
		.await
}

#[rstest::rstest]
#[tokio::test]
async fn failed_admission_with_unavailable_release_expires_at_home(
	#[future(awt)]
	#[from(test_environment)]
	test_environment: Arc<TestEnvironment>,
) {
	let fixture = RemoteFixture::new(&test_environment).await;
	let run = &fixture.run;
	let key = format!("human:{}:{}", run.id, Uuid::new_v4());
	fixture.outage.store(true, Ordering::SeqCst);
	assert!(
		fixture
			.executor
			.admit_run_message(run, "human", "rejected correction", &key, 1)
			.await
			.is_err()
	);
	assert!(
		fixture
			.executor
			.store
			.run_inputs(run.id)
			.await
			.unwrap()
			.is_empty()
	);
	let leased: bool = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::col(Alias::new("expires_at")).is_not_null())
			.from(Alias::new("remote_run_message_fences"))
			.and_where(Expr::cust("task_id = $1 AND idempotency_key = $2"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.task_id)
	.bind(&key)
	.fetch_one(&fixture.home.store.pool)
	.await
	.unwrap();
	assert!(
		leased,
		"failed admission must leave only a leased reservation"
	);
	sqlx::query(
		&Query::update()
			.table(Alias::new("remote_run_message_fences"))
			.value(
				Alias::new("expires_at"),
				Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 second'"),
			)
			.and_where(Expr::col(Alias::new("task_id")).eq(uuid_expr(run.task_id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&fixture.home.store.pool)
	.await
	.unwrap();
	let task = fixture.home.store.task(run.task_id).await.unwrap();
	let cancelled = fixture
		.home
		.store
		.transition(
			task.id,
			task.revision,
			task.owner.as_deref().unwrap(),
			"CANCELLED",
		)
		.await
		.unwrap();
	assert_eq!(cancelled.status, "CANCELLED");
	fixture.cleanup().await;
}

struct RemoteFixture {
	home: Federation,
	executor: Federation,
	home_url: String,
	home_schema: String,
	executor_url: String,
	executor_schema: String,
	run: Run,
	outage: Arc<AtomicBool>,
	server: tokio::task::JoinHandle<()>,
}

impl RemoteFixture {
	async fn new(environment: &TestEnvironment) -> Self {
		let (mut home, home_url, home_schema) = setup(environment).await;
		let (mut executor, executor_url, executor_schema) = setup(environment).await;
		executor.config.node_id = "aidash://fence-executor".into();
		executor.store.node_id = executor.config.node_id.clone();
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		home.config.endpoint = format!("http://{}", listener.local_addr().unwrap());
		let outage = Arc::new(AtomicBool::new(false));
		let home_app = api::router(home.clone()).layer(axum::middleware::from_fn_with_state(
			outage.clone(),
			promotion_outage,
		));
		let executor_app = api::router(executor.clone());
		bootstrap(&home, &home_app, "http://127.0.0.1:9").await;
		bootstrap(&executor, &executor_app, "http://127.0.0.1:9").await;
		add_peer(&home, &executor.config.node_id, "http://127.0.0.1:9").await;
		add_peer(&executor, &home.config.node_id, &home.config.endpoint).await;
		let server = tokio::spawn(async move { axum::serve(listener, home_app).await.unwrap() });
		let workspace = home
			.store
			.create_workspace("Remote fences", "Run-message recovery")
			.await
			.unwrap();
		let task = home
			.store
			.create_task(
				workspace.id,
				&NewTask {
					title: "Remote correction".into(),
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
		let owner = qualified_agent(&executor.config.node_id, &agent.id, &agent.version);
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
					uuid_expr(task.id),
					Expr::value(executor.config.node_id.clone()),
					Expr::value(agent.id.clone()),
					Expr::value(agent.version.clone()),
				])
				.to_string(PostgresQueryBuilder),
		)
		.execute(&home.store.pool)
		.await
		.unwrap();
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
		Self {
			home,
			executor,
			home_url,
			home_schema,
			executor_url,
			executor_schema,
			run,
			outage,
			server,
		}
	}

	async fn cleanup(self) {
		self.server.abort();
		cleanup(self.home, &self.home_url, &self.home_schema).await;
		cleanup(self.executor, &self.executor_url, &self.executor_schema).await;
	}
}

#[rstest::rstest]
#[tokio::test]
async fn admitted_input_recovers_promotion_after_expiry_and_terminal_home(
	#[future(awt)]
	#[from(test_environment)]
	test_environment: Arc<TestEnvironment>,
) {
	let fixture = RemoteFixture::new(&test_environment).await;
	let run = &fixture.run;
	let key = format!("human:{}:{}", run.id, Uuid::new_v4());
	fixture.outage.store(true, Ordering::SeqCst);
	assert!(
		fixture
			.executor
			.admit_run_message(
				run,
				"human",
				"durable correction",
				&key,
				fixture.executor.run_message_limit(run).await.unwrap()
			)
			.await
			.is_err()
	);
	assert_eq!(
		fixture
			.executor
			.store
			.run_inputs(run.id)
			.await
			.unwrap()
			.len(),
		1
	);
	sqlx::query(
		&Query::update()
			.table(Alias::new("remote_run_message_fences"))
			.value(
				Alias::new("expires_at"),
				Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 second'"),
			)
			.and_where(Expr::col(Alias::new("task_id")).eq(uuid_expr(run.task_id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&fixture.home.store.pool)
	.await
	.unwrap();
	let task = fixture.home.store.task(run.task_id).await.unwrap();
	fixture
		.home
		.store
		.transition(
			task.id,
			task.revision,
			task.owner.as_deref().unwrap(),
			"CANCELLED",
		)
		.await
		.unwrap();
	fixture.outage.store(false, Ordering::SeqCst);
	fixture
		.executor
		.deliver_run_messages(run)
		.await
		.expect("a durable executor input must recover its exact expired reservation");
	let inputs = fixture.executor.store.run_inputs(run.id).await.unwrap();
	assert!(inputs[0].message_id.is_some());
	for (proof_run, proof_seq, proof_content) in [
		(run.id, inputs[0].seq + 1, "durable correction"),
		(run.id, 0, "durable correction"),
		(run.id, inputs[0].seq, "different correction"),
		(Uuid::new_v4(), inputs[0].seq, "durable correction"),
	] {
		let result = fixture
			.executor
			.request::<Value>(
				&fixture.home.config.node_id,
				reqwest::Method::POST,
				"/workspace",
				Some(&json!({
					"task_id": run.task_id, "agent":{"id":run.agent_id, "version":run.agent_version},
					"operation":"run_message_commit", "data":{
						"run_id":proof_run, "key":key, "content":proof_content, "input_seq":proof_seq
					}
				})),
			)
			.await;
		assert!(
			result.is_err(),
			"promotion proof must bind a positive immutable sequence, run and content"
		);
	}
	assert_eq!(
		fixture
			.home
			.store
			.snapshot(run.workspace_id)
			.await
			.unwrap()
			.messages
			.iter()
			.filter(|message| message.content == "durable correction")
			.count(),
		1
	);
	fixture
		.executor
		.admit_run_message(
			run,
			"human",
			"durable correction",
			&key,
			fixture.executor.run_message_limit(run).await.unwrap(),
		)
		.await
		.unwrap();
	assert!(
		fixture
			.executor
			.admit_run_message(
				run,
				"human",
				"different content",
				&key,
				fixture.executor.run_message_limit(run).await.unwrap()
			)
			.await
			.is_err()
	);
	let fresh_key = format!("human:{}:{}", run.id, Uuid::new_v4());
	assert!(
		fixture
			.executor
			.admit_run_message(
				run,
				"human",
				"not previously admitted",
				&fresh_key,
				fixture.executor.run_message_limit(run).await.unwrap()
			)
			.await
			.is_err()
	);
	Harness {
		federation: fixture.executor.clone(),
	}
	.worker_once()
	.await
	.unwrap();
	assert_eq!(
		fixture.executor.store.run(run.id).await.unwrap().phase,
		"CANCELLED"
	);
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn scoped_remote_admission_imports_history_before_assigning_new_sequence(
	#[future(awt)]
	#[from(test_environment)]
	test_environment: Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&test_environment).await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(&app, &token, "POST", "/api/conversations", json!({
		"title":"Scoped history", "goal":"Reply", "target":{"id":"research","version":"1.0.0"}, "target_kind":"agent"
	})).await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	let old_message = Message {
		id: Uuid::new_v4(),
		workspace_id: run.workspace_id,
		sender: format!("human@{}", f.config.node_id),
		content: "older correction".into(),
		idempotency_key: Some(format!(
			"{}:{}:human:{}:{}",
			f.config.node_id,
			run.task_id,
			run.id,
			Uuid::new_v4()
		)),
		created_at: chrono::Utc::now() - chrono::Duration::minutes(1),
	};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let remote = Router::new().route(
		"/federation/v0.1/workspace",
		post(move |Json(command): Json<Value>| {
			let old_message = old_message.clone();
			async move {
				match command["operation"].as_str().unwrap() {
					"run_message_delivery_capability" => {
						(StatusCode::OK, Json(json!({"protocol":2})))
					}
					"run_message_history" => (StatusCode::OK, Json(json!([old_message]))),
					"run_message_reserve" => (StatusCode::OK, Json(json!({"reserved":true}))),
					"run_message_commit" => (StatusCode::OK, Json(json!({"committed":true}))),
					"run_message_delivery" => (
						StatusCode::SERVICE_UNAVAILABLE,
						Json(json!({"error":"delivery pending"})),
					),
					_ => (
						StatusCode::BAD_REQUEST,
						Json(json!({"error":"unknown federation operation"})),
					),
				}
			}
		}),
	);
	let server = tokio::spawn(async move { axum::serve(listener, remote).await.unwrap() });
	add_peer(&f, "aidash://scoped-history-home", &endpoint).await;
	sqlx::query(
		&Query::update()
			.table(Alias::new("runs"))
			.value(
				Alias::new("home_node"),
				Expr::value("aidash://scoped-history-home"),
			)
			.and_where(Expr::col(Alias::new("id")).eq(uuid_expr(run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let (status, response) = request(
		&app,
		&token,
		"POST",
		&format!("/api/runs/{}/message", run.id),
		json!({"content":"newer correction", "idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	let inputs = f.store.run_inputs(run.id).await.unwrap();
	assert_eq!(
		inputs.len(),
		2,
		"scoped admission must atomically import older home history"
	);
	assert_eq!(inputs[0].content, "older correction");
	assert_eq!(inputs[1].content, "newer correction");
	assert!(inputs[0].seq < inputs[1].seq);
	server.abort();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn migration_backfills_active_remote_fences_before_legacy_effects(
	#[future(awt)]
	#[from(test_environment)]
	test_environment: Arc<TestEnvironment>,
) {
	let fixture = RemoteFixture::new(&test_environment).await;
	let db =
		sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(fixture.home.store.pool.clone());
	let migrations = Migrator::migrations();
	let gate = migrations
		.iter()
		.position(|migration| migration.name() == "m20260924_080000_legacy_federated_input_gate")
		.expect("legacy federated input gate migration");
	Migrator::down(&db, Some((migrations.len() - gate) as u32))
		.await
		.unwrap();
	let run = &fixture.run;
	for key in [
		format!("human:{}:{}", run.id, Uuid::new_v4()),
		format!("subject-human:acme:alice:{}:{}", run.id, Uuid::new_v4()),
	] {
		fixture
			.home
			.store
			.message(
				run.workspace_id,
				&format!("human@{}", fixture.executor.config.node_id),
				"accepted before upgrade",
				Some(&format!(
					"{}:{}:{key}",
					fixture.executor.config.node_id, run.task_id
				)),
			)
			.await
			.unwrap();
	}
	// A similarly prefixed but malformed key must neither crash the migration
	// nor create a fence for an unrelated run.
	fixture
		.home
		.store
		.message(
			run.workspace_id,
			&format!("human@{}", fixture.executor.config.node_id),
			"not a run correction",
			Some(&format!(
				"{}:{}:human:not-a-uuid:not-a-uuid",
				fixture.executor.config.node_id, run.task_id
			)),
		)
		.await
		.unwrap();
	let terminal = fixture
		.home
		.store
		.create_task(
			run.workspace_id,
			&NewTask {
				title: "Already cancelled".into(),
				description: "Do not fence terminal history".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"human",
			None,
		)
		.await
		.unwrap();
	fixture
		.home
		.store
		.transition(terminal.id, terminal.revision, "human", "CANCELLED")
		.await
		.unwrap();
	fixture
		.home
		.store
		.message(
			run.workspace_id,
			&format!("human@{}", fixture.executor.config.node_id),
			"terminal history",
			Some(&format!(
				"{}:{}:human:{}:{}",
				fixture.executor.config.node_id,
				terminal.id,
				Uuid::new_v4(),
				Uuid::new_v4()
			)),
		)
		.await
		.unwrap();
	fixture
		.home
		.store
		.message(
			run.workspace_id,
			"human@aidash://other-executor",
			"mismatched sender",
			Some(&format!(
				"{}:{}:human:{}:{}",
				fixture.executor.config.node_id,
				run.task_id,
				run.id,
				Uuid::new_v4()
			)),
		)
		.await
		.unwrap();
	Migrator::up(&db, None).await.unwrap();
	let total: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("remote_run_message_fences"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&fixture.home.store.pool)
	.await
	.unwrap();
	assert_eq!(
		total, 2,
		"only active, correctly peer-bound run corrections are backfilled"
	);
	let fences: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("remote_run_message_fences"))
			.and_where(Expr::col(Alias::new("task_id")).eq(uuid_expr(run.task_id)))
			.and_where(Expr::col(Alias::new("run_id")).eq(uuid_expr(run.id)))
			.and_where(Expr::cust("expires_at IS NULL AND NOT consumed"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&fixture.home.store.pool)
	.await
	.unwrap();
	assert_eq!(
		fences, 2,
		"pre-upgrade corrections must be fenced immediately on the home node"
	);
	assert!(
		fixture
			.home
			.store
			.message(
				run.workspace_id,
				"agent",
				"stale legacy output",
				Some(&format!(
					"{}:{}:{}:0:output",
					fixture.executor.config.node_id, run.task_id, run.id
				))
			)
			.await
			.is_err()
	);
	let task = fixture.home.store.task(run.task_id).await.unwrap();
	assert!(
		fixture
			.home
			.store
			.transition(
				task.id,
				task.revision,
				task.owner.as_deref().unwrap(),
				"CANCELLED"
			)
			.await
			.is_err()
	);
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn terminal_rpc_is_bounded_for_large_historical_ledgers(
	#[future(awt)]
	#[from(test_environment)]
	test_environment: Arc<TestEnvironment>,
) {
	let fixture = RemoteFixture::new(&test_environment).await;
	let run = &fixture.run;
	// These are already-imported pageable references, which legitimately do not
	// share the inline-content quota. Seed in one statement to keep the test fast.
	let mut insert = Query::insert();
	insert.into_table(Alias::new("run_inputs")).columns([
		Alias::new("run_id"),
		Alias::new("sender"),
		Alias::new("content"),
		Alias::new("idempotency_key"),
		Alias::new("message_id"),
		Alias::new("reference_only"),
	]);
	for _ in 0..14_000 {
		insert.values_panic([
			uuid_expr(run.id),
			Expr::value("human"),
			Expr::value("historical correction"),
			Expr::value(format!("human:{}:{}", run.id, Uuid::new_v4())),
			uuid_expr(Uuid::new_v4()),
			Expr::value(true),
		]);
	}
	sqlx::query(&insert.to_string(PostgresQueryBuilder))
		.execute(&fixture.executor.store.pool)
		.await
		.unwrap();
	let task = fixture
		.executor
		.transition_terminal_run_messages(run, "FAILED")
		.await
		.expect("terminal RPC size must not grow with the imported input ledger");
	assert_eq!(task.status, "FAILED");
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn bounded_terminal_transition_keeps_unadmitted_reservations_and_rolls_back_consumption(
	#[future(awt)]
	#[from(test_environment)]
	test_environment: Arc<TestEnvironment>,
) {
	let fixture = RemoteFixture::new(&test_environment).await;
	let run = &fixture.run;
	let key = format!("human:{}:{}", run.id, Uuid::new_v4());
	fixture
		.executor
		.admit_run_message(
			run,
			"human",
			"known correction",
			&key,
			fixture.executor.run_message_limit(run).await.unwrap(),
		)
		.await
		.unwrap();
	fixture.executor.deliver_run_messages(run).await.unwrap();
	let unknown_key = format!("human:{}:{}", run.id, Uuid::new_v4());
	fixture
		.home
		.store
		.reserve_remote_run_message(
			run.task_id,
			run.id,
			&fixture.executor.config.node_id,
			&unknown_key,
			"concurrent admission",
		)
		.await
		.unwrap();
	assert!(
		fixture
			.executor
			.transition_terminal_run_messages(run, "CANCELLED")
			.await
			.is_err(),
		"an unadmitted home reservation must not be consumed by the executor's watermark"
	);
	let consumed: bool = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("consumed"))
			.from(Alias::new("remote_run_message_fences"))
			.and_where(Expr::col(Alias::new("task_id")).eq(uuid_expr(run.task_id)))
			.and_where(Expr::col(Alias::new("idempotency_key")).eq(key.clone()))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&fixture.home.store.pool)
	.await
	.unwrap();
	assert!(
		!consumed,
		"fence consumption must roll back with the blocked task transition"
	);
	assert_eq!(
		fixture.home.store.task(run.task_id).await.unwrap().status,
		"RUNNING"
	);
	fixture
		.home
		.store
		.release_remote_run_message(
			run.task_id,
			run.id,
			&fixture.executor.config.node_id,
			&[unknown_key],
		)
		.await
		.unwrap();
	let task = fixture
		.executor
		.transition_terminal_run_messages(run, "CANCELLED")
		.await
		.unwrap();
	assert_eq!(task.status, "CANCELLED");
	fixture.cleanup().await;
}
