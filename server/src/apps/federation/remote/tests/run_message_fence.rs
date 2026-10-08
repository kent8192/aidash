#[path = "../../../execution/tests/support/legacy.rs"]
mod common;

use aidash_server::{
	domain::{Message, NewTask, Run, qualified_agent},
	federation::{Federation, Home},
	harness::Harness,
};
use common::upstream_fixtures;
use common::{bootstrap, cleanup, request};
use http::StatusCode;
use reinhardt::ServerRouter as Router;
use reinhardt::http::{Handler, Middleware, ViewResult};
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use reinhardt::{Request, Response};
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicBool, Ordering},
};
use upstream_fixtures::handler;
use uuid::Uuid;

fn uuid_expr(id: Uuid) -> reinhardt::query::SimpleExpr {
	Expr::cust(format!("'{id}'::uuid")).into()
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
				IntoValue::into_value(node),
				IntoValue::into_value(endpoint),
				IntoValue::into_value("AIDASH_SECRET_TEST_PEER"),
				IntoValue::into_value("0.1"),
				IntoValue::into_value(true),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.pool.driver())
	.await
	.unwrap();
}

struct PromotionOutage(Arc<AtomicBool>);
#[async_trait::async_trait]
impl Middleware for PromotionOutage {
	async fn process(&self, request: Request, next: Arc<dyn Handler>) -> ViewResult<Response> {
		if self.0.load(Ordering::SeqCst) && request.uri.path().ends_with("/workspace") {
			let command: Value = serde_json::from_slice(request.body()).unwrap();
			if matches!(
				command["operation"].as_str(),
				Some("run_message_commit" | "run_message_release")
			) {
				return Response::new(StatusCode::SERVICE_UNAVAILABLE)
					.with_json(&json!({"error":"promotion acknowledgement unavailable"}));
			}
		}
		next.handle(request).await
	}
}

#[rstest::rstest]
#[tokio::test]
async fn failed_admission_with_unavailable_release_expires_at_home(
	#[future(awt)] remote_fixture: RemoteFixture,
) {
	let fixture = remote_fixture;
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
	let leased: bool = {
		let query_bind_1 = run.task_id;
		let query_bind_2 = &key;
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::col(Alias::new("expires_at")).is_not_null())
				.from(Alias::new("remote_run_message_fences"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ? AND idempotency_key = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(fixture.home.store.pool.driver())
		.await
	}
	.unwrap();
	assert!(
		leased,
		"failed admission must leave only a leased reservation"
	);
	sqlx::query(
		&Query::update()
			.table(Alias::new("remote_run_message_fences"))
			.value_expr(
				Alias::new("expires_at"),
				Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 second'"),
			)
			.and_where(Expr::col(Alias::new("task_id")).eq(uuid_expr(run.task_id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(fixture.home.store.pool.driver())
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
			aidash_server::domain::TaskStatus::Cancelled,
		)
		.await
		.unwrap();
	assert_eq!(cancelled.status.as_str(), "CANCELLED");
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
	server: common::TestApplication,
	_peer_owners: Vec<common::PeerFixture>,
}

#[rstest::fixture]
fn promotion_outage() -> Arc<AtomicBool> {
	Arc::new(AtomicBool::new(false))
}
#[rstest::fixture]
fn promotion_transform(promotion_outage: Arc<AtomicBool>) -> common::RouterTransform {
	Arc::new(move |router| router.with_middleware(PromotionOutage(promotion_outage.clone())))
}
#[rstest::fixture]
async fn remote_fixture(
	#[from(promotion_outage)] outage: Arc<AtomicBool>,
	#[from(promotion_transform)]
	#[with(outage.clone())]
	_transform: common::RouterTransform,
	#[future(awt)]
	#[from(common::native_peer)]
	#[with("aidash://execution-test",_transform.clone())]
	home_fixture: common::PeerFixture,
	#[future(awt)]
	#[from(common::native_peer)]
	#[with("aidash://fence-executor")]
	executor_fixture: common::PeerFixture,
) -> RemoteFixture {
	let (home, home_url, home_schema) = home_fixture.runtime.parts();
	let (executor, executor_url, executor_schema) = executor_fixture.runtime.parts();
	let home_app = home_fixture.application.clone();
	let executor_app = executor_fixture.application.clone();
	bootstrap(&home, &home_app, "http://127.0.0.1:9").await;
	bootstrap(&executor, &executor_app, "http://127.0.0.1:9").await;
	add_peer(&home, &executor.config.node_id, "http://127.0.0.1:9").await;
	add_peer(&executor, &home.config.node_id, &home.config.endpoint).await;
	let server = home_app;
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
			.from_subquery(
				reinhardt::query::Query::select()
					.expr(uuid_expr(task.id))
					.expr(Expr::value(executor.config.node_id.clone()))
					.expr(Expr::value(agent.id.clone()))
					.expr(Expr::value(agent.version.clone()))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(home.store.pool.driver())
	.await
	.unwrap();
	let task = home
		.store
		.claim(task.id, task.revision, &owner, &agent)
		.await
		.unwrap();
	let task = home
		.store
		.transition(
			task.id,
			task.revision,
			&owner,
			aidash_server::domain::TaskStatus::Running,
		)
		.await
		.unwrap();
	let run = executor
		.store
		.accept_run(&task, &home.config.node_id, &agent.id, &agent.version)
		.await
		.unwrap();
	RemoteFixture {
		home,
		executor,
		home_url,
		home_schema,
		executor_url,
		executor_schema,
		run,
		outage,
		server,
		_peer_owners: vec![home_fixture, executor_fixture],
	}
}
impl RemoteFixture {
	async fn cleanup(self) {
		drop(self.server);
		cleanup(self.home, &self.home_url, &self.home_schema).await;
		cleanup(self.executor, &self.executor_url, &self.executor_schema).await;
	}
}

#[rstest::rstest]
#[tokio::test]
async fn admitted_input_recovers_promotion_after_expiry_and_terminal_home(
	#[future(awt)] remote_fixture: RemoteFixture,
) {
	let fixture = remote_fixture;
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
			.value_expr(
				Alias::new("expires_at"),
				Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 second'"),
			)
			.and_where(Expr::col(Alias::new("task_id")).eq(uuid_expr(run.task_id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(fixture.home.store.pool.driver())
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
			aidash_server::domain::TaskStatus::Cancelled,
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
		fixture
			.executor
			.store
			.run(run.id)
			.await
			.unwrap()
			.phase()
			.as_str(),
		"CANCELLED"
	);
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn scoped_remote_admission_imports_history_before_assigning_new_sequence(
	history: HistoryFuture,
	#[from(history_router)]
	#[with(history.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[future(awt)]
	#[from(upstream_fixtures::async_upstream)]
	#[with(_router.clone())]
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	let state = history.await;
	let (f, url, schema) = state.fixture.runtime.parts();
	let app = state.fixture.application.clone();
	let token = state.token;
	let run = state.run;
	let endpoint = server.url.clone();

	add_peer(&f, "aidash://scoped-history-home", &endpoint).await;
	sqlx::query(
		&Query::update()
			.table(Alias::new("runs"))
			.value_expr(
				Alias::new("home_node"),
				Expr::value("aidash://scoped-history-home"),
			)
			.and_where(Expr::col(Alias::new("id")).eq(uuid_expr(run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.pool.driver())
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
	drop(server);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn durable_remote_admission_fences_both_human_key_formats(
	#[future(awt)] remote_fixture: RemoteFixture,
) {
	let fixture = remote_fixture;
	let run = &fixture.run;
	let home = Home::new(fixture.executor.clone(), run.clone());
	let limit = fixture.executor.run_message_limit(run).await.unwrap();
	let keys = [
		format!("human:{}:{}", run.id, Uuid::new_v4()),
		format!("subject-human:acme:alice:{}:{}", run.id, Uuid::new_v4()),
	];
	for key in &keys {
		fixture
			.executor
			.admit_run_message(run, "human", "durable correction", key, limit)
			.await
			.unwrap();
	}
	fixture.executor.deliver_run_messages(run).await.unwrap();
	let history = home.historical_run_messages().await.unwrap();
	assert_eq!(history.len(), 2);
	assert!(
		history
			.iter()
			.all(|message| message.content == "durable correction")
	);
	// Replays stay idempotent and a reused key cannot change persisted content.
	fixture
		.executor
		.admit_run_message(run, "human", "durable correction", &keys[0], limit)
		.await
		.unwrap();
	assert!(
		fixture
			.executor
			.admit_run_message(run, "human", "changed correction", &keys[0], limit)
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
		2
	);
	assert!(
		home.reserve_run_message("human:not-a-uuid:not-a-uuid", "invalid key")
			.await
			.is_err()
	);
	assert!(
		home.message(&format!("{}:0:output", run.id), "stale output")
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
				aidash_server::domain::TaskStatus::Cancelled
			)
			.await
			.is_err()
	);
	let cancelled = fixture
		.executor
		.transition_terminal_run_messages(run, aidash_server::domain::TaskStatus::Cancelled)
		.await
		.unwrap();
	assert_eq!(
		cancelled.status,
		aidash_server::domain::TaskStatus::Cancelled
	);
	assert_eq!(
		fixture
			.executor
			.store
			.run_inputs(run.id)
			.await
			.unwrap()
			.len(),
		2
	);
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn terminal_rpc_is_bounded_for_large_historical_ledgers(
	#[future(awt)] remote_fixture: RemoteFixture,
) {
	let fixture = remote_fixture;
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
		insert.from_subquery(
			reinhardt::query::Query::select()
				.expr(uuid_expr(run.id))
				.expr(Expr::value("human"))
				.expr(Expr::value("historical correction"))
				.expr(Expr::value(format!("human:{}:{}", run.id, Uuid::new_v4())))
				.expr(uuid_expr(Uuid::new_v4()))
				.expr(Expr::value(true))
				.to_owned(),
		);
	}
	sqlx::query(&insert.to_string(PostgresQueryBuilder))
		.execute(fixture.executor.store.pool.driver())
		.await
		.unwrap();
	let task = fixture
		.executor
		.transition_terminal_run_messages(run, aidash_server::domain::TaskStatus::Failed)
		.await
		.expect("terminal RPC size must not grow with the imported input ledger");
	assert_eq!(task.status.as_str(), "FAILED");
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn bounded_terminal_transition_keeps_unadmitted_reservations_and_rolls_back_consumption(
	#[future(awt)] remote_fixture: RemoteFixture,
) {
	let fixture = remote_fixture;
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
			.transition_terminal_run_messages(run, aidash_server::domain::TaskStatus::Cancelled)
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
	.fetch_one(fixture.home.store.pool.driver())
	.await
	.unwrap();
	assert!(
		!consumed,
		"fence consumption must roll back with the blocked task transition"
	);
	assert_eq!(
		fixture
			.home
			.store
			.task(run.task_id)
			.await
			.unwrap()
			.status
			.as_str(),
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
		.transition_terminal_run_messages(run, aidash_server::domain::TaskStatus::Cancelled)
		.await
		.unwrap();
	assert_eq!(task.status.as_str(), "CANCELLED");
	fixture.cleanup().await;
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

use reinhardt::query::IntoValue;

use reinhardt::query::SimpleExpr;

#[derive(Clone)]
struct HistoryState {
	fixture: common::ApplicationFixture,
	token: String,
	run: Run,
	old_message: Message,
}
type HistoryFuture =
	futures_util::future::Shared<futures_util::future::BoxFuture<'static, HistoryState>>;
#[rstest::fixture]
fn history(
	#[from(common::native_application)] application: common::ApplicationFuture,
) -> HistoryFuture {
	use futures_util::FutureExt;
	async move {
let fixture=application.await;

	let (f,_,_)=fixture.runtime.parts();
let app=fixture.application.clone();
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

HistoryState {fixture,token,run,old_message}
}.boxed().shared()
}
#[rstest::fixture]
fn history_router(history: HistoryFuture) -> upstream_fixtures::RouterFuture {
	use futures_util::FutureExt;
	async move {
		let old_message = history.await.old_message;
		Arc::new(Router::new().handler(
			"/federation/v0.1/workspace",
			handler(http::Method::POST, move |request: reinhardt::Request| {
				let command = request.json::<Value>().unwrap();
				let old_message = old_message.clone();
				async move {
					match command["operation"].as_str().unwrap() {
						"run_message_delivery_capability" => {
							reinhardt::Response::new(StatusCode::OK)
								.with_json(&json!({"protocol":2}))
								.unwrap()
						}
						"run_message_history" => reinhardt::Response::new(StatusCode::OK)
							.with_json(&json!([old_message]))
							.unwrap(),
						"run_message_reserve" => reinhardt::Response::new(StatusCode::OK)
							.with_json(&json!({"reserved":true}))
							.unwrap(),
						"run_message_commit" => reinhardt::Response::new(StatusCode::OK)
							.with_json(&json!({"committed":true}))
							.unwrap(),
						"run_message_delivery" => {
							reinhardt::Response::new(StatusCode::SERVICE_UNAVAILABLE)
								.with_json(&json!({"error":"delivery pending"}))
								.unwrap()
						}
						_ => reinhardt::Response::new(StatusCode::BAD_REQUEST)
							.with_json(&json!({"error":"unknown federation operation"}))
							.unwrap(),
					}
				}
			}),
		))
	}
	.boxed()
	.shared()
}
