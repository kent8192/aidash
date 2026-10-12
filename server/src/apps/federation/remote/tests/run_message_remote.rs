#[path = "../../../execution/tests/support/legacy.rs"]
mod common;

use aidash_server::{
	domain::{ArtifactInput, NewTask, Run, qualified_agent},
	federation::{Federation, Home},
	harness::Harness,
};
use common::{bootstrap, cleanup, request};
use http::StatusCode;
use reinhardt::http::{Handler, Middleware, ViewResult};
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use reinhardt::{Request, Response};
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;

async fn peer_control(app: &common::TestApplication, node: &str, body: Value) -> (u16, Value) {
	let authorization = format!(
		"Bearer {}",
		std::env::var("AIDASH_SECRET_TEST_PEER").unwrap()
	);
	let response = async {
		let client = &(app.client());
		let mut request = client
			.request(http::Method::POST, "/federation/v0.1/control")
			.body(bytes::Bytes::copy_from_slice(body.to_string().as_bytes()))
			.header(http::header::CONTENT_TYPE, "application/json");
		for (name, value) in &[
			("authorization", authorization.as_str()),
			("x-aidash-node", node),
			("x-aidash-protocol", "0.2"),
		] {
			request = request.header(*name, *value);
		}
		request.send().await
	}
	.await
	.unwrap();
	(
		response.status_code(),
		serde_json::from_slice(response.body()).unwrap_or(Value::Null),
	)
}

async fn add_peer(f: &Federation, node: &str, endpoint: &str) {
	{
		let query_bind_1 = node;
		let query_bind_2 = endpoint;
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
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(Expr::cust("'AIDASH_SECRET_TEST_PEER'"))
						.expr(Expr::cust("'0.2'"))
						.expr(Expr::cust("TRUE"))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
	}
	.unwrap();
}

// Simulate durable home delivery surviving executor history loss. This uses
// the production reservation and delivery endpoints on the current schema.
async fn delivered_history(executor: &Federation, run: &Run, key: Uuid, content: &str) {
	let home = Home::new(executor.clone(), run.clone());
	let key = format!("human:{}:{key}", run.id);
	assert!(home.reserve_run_message(&key, content).await.unwrap());
	home.human_message_record(&key, content).await.unwrap();
}

struct PeerMode {
	old_peer: AtomicBool,
	delivery_outage: AtomicBool,
	commit_response_lost: AtomicBool,
}

struct PeerFaults(Arc<PeerMode>);

#[async_trait::async_trait]
impl Middleware for PeerFaults {
	async fn process(&self, request: Request, next: Arc<dyn Handler>) -> ViewResult<Response> {
		let mode = &self.0;
		if !request.uri.path().ends_with("/workspace")
			|| (!mode.old_peer.load(Ordering::SeqCst)
				&& !mode.delivery_outage.load(Ordering::SeqCst)
				&& !mode.commit_response_lost.load(Ordering::SeqCst))
		{
			return next.handle(request).await;
		}
		let command: Value = serde_json::from_slice(request.body()).unwrap();
		let operation = command["operation"].as_str().unwrap();
		if mode.delivery_outage.load(Ordering::SeqCst) && operation == "run_message_delivery" {
			return Response::new(StatusCode::SERVICE_UNAVAILABLE)
				.with_json(&json!({"error":"simulated delivery outage"}));
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
					| "run_message_terminal_transition"
			) {
			return Response::new(StatusCode::BAD_REQUEST)
				.with_json(&json!({"error":"unknown federation operation"}));
		}
		let response = next.handle(request).await?;
		if mode.commit_response_lost.load(Ordering::SeqCst)
			&& operation == "run_message_commit"
			&& response.status.is_success()
		{
			return Response::new(StatusCode::SERVICE_UNAVAILABLE)
				.with_json(&json!({"error":"simulated lost commit response"}));
		}
		if mode.old_peer.load(Ordering::SeqCst)
			&& operation == "human_message"
			&& response.status.is_success()
		{
			return Response::ok().with_json(&json!({"sent":true}));
		}
		Ok(response)
	}
}

#[rstest::rstest]
#[tokio::test]
async fn committed_fence_survives_delayed_release_and_terminal_transition_is_atomic(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(
		&app,
		&token,
		"POST",
		"/api/conversations",
		json!({"title":"Atomic terminal transition","goal":"Reply","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	assert!(
		(Harness {
			federation: f.clone()
		})
		.worker_once()
		.await
		.unwrap()
	);
	let task = f.store.task(run.task_id).await.unwrap();
	let owner = task.owner.clone().unwrap();
	let key = format!("human:{}:{}", run.id, Uuid::new_v4());
	f.store
		.reserve_remote_run_message(task.id, run.id, "aidash://executor", &key, "correction")
		.await
		.unwrap();
	f.store
		.commit_remote_run_message(task.id, run.id, &key, "correction")
		.await
		.unwrap();
	// Model the executor binding the durable input sequence after local admission.
	{
		let query_bind_1 = task.id;
		let query_bind_2 = run.id;
		let query_bind_3 = &key;
		sqlx::query(
			&Query::update()
				.table(Alias::new("remote_run_message_fences"))
				.value_expr(Alias::new("input_seq"), Expr::cust("1"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
						Expr::value(query_bind_3.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
	}
	.unwrap();
	// A rejection response from an earlier overlapping attempt can arrive after
	// the successful commit. It must not delete the non-expiring fence.
	f.store
		.release_remote_run_message(
			task.id,
			run.id,
			"aidash://executor",
			std::slice::from_ref(&key),
		)
		.await
		.unwrap();
	let retained: bool = sqlx::query_scalar(
		"SELECT EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = $1 AND run_id = $2 AND idempotency_key = $3 AND expires_at IS NULL AND NOT consumed)",
	)
	.bind(task.id)
	.bind(run.id)
	.bind(&key)
	.fetch_one(f.store.pool.driver())
	.await
	.unwrap();
	assert!(retained);
	assert!(
		f.store
			.transition(
				task.id,
				task.revision,
				&owner,
				aidash_server::domain::TaskStatus::Cancelled
			)
			.await
			.is_err(),
		"the committed correction must fence an ordinary terminal transition"
	);
	let cancelled = f
		.store
		.transition_remote_run_message_terminal(
			task.id,
			task.revision,
			&owner,
			aidash_server::domain::TaskStatus::Cancelled,
			run.id,
			std::slice::from_ref(&key),
		)
		.await
		.unwrap();
	assert_eq!(cancelled.status.as_str(), "CANCELLED");
	let consumed: bool = sqlx::query_scalar(
		"SELECT consumed FROM remote_run_message_fences WHERE task_id = $1 AND idempotency_key = $2",
	)
	.bind(task.id)
	.bind(&key)
	.fetch_one(f.store.pool.driver())
	.await
	.unwrap();
	assert!(consumed);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn peer_prefixed_legacy_output_checks_remote_fence_without_a_local_run(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	// Retain the disposable environment through the remote fence assertions.
	let runtime_owner = runtime.await;
	let (f, url, schema) = runtime_owner.parts();
	let workspace = f
		.store
		.create_workspace("Remote output fence", "No executor run exists at home")
		.await
		.unwrap();
	let task = f
		.store
		.create_task(
			workspace.id,
			&NewTask {
				title: "Remote task".into(),
				description: "Fence stale output".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"human",
			None,
		)
		.await
		.unwrap();
	let remote_run = Uuid::new_v4();
	let input_key = format!("human:{remote_run}:{}", Uuid::new_v4());
	f.store
		.reserve_remote_run_message(
			task.id,
			remote_run,
			"aidash://executor",
			&input_key,
			"remote correction",
		)
		.await
		.unwrap();
	f.store
		.commit_remote_run_message(task.id, remote_run, &input_key, "remote correction")
		.await
		.unwrap();
	let peer_output_key = format!("aidash://executor:{}:{remote_run}:0:output", task.id);
	assert!(
		f.store
			.message(
				workspace.id,
				"agent@aidash://executor",
				"stale peer output",
				Some(&peer_output_key),
			)
			.await
			.is_err(),
		"the database fence must not depend on a home-side executor run row"
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn remote_history_references_can_span_multiple_inference_pages(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(
		&app,
		&token,
		"POST",
		"/api/conversations",
		json!({"title":"Paged remote history","goal":"Reply","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	let mut history = Vec::new();
	for index in 0..20 {
		let key = format!("human:{}:{}", run.id, Uuid::new_v4());
		let message = f
			.store
			.message_record(
				run.workspace_id,
				"human@remote",
				&format!("history {index}: {}", "x".repeat(4096)),
				Some(&format!("remote-history-{index}")),
			)
			.await
			.unwrap();
		history.push((key, message));
	}
	f.store
		.import_remote_run_messages(run.id, &history, 512)
		.await
		.expect("pageable references are not subject to one aggregate page cap");
	let inputs = f.store.run_inputs(run.id).await.unwrap();
	assert_eq!(inputs.len(), history.len());
	assert!(inputs.iter().all(|input| input.reference_only));
	let new_key = format!("human:{}:{}", run.id, Uuid::new_v4());
	f.store
		.accept_run_message(run.id, "human", "latest correction", &new_key, 512)
		.await
		.expect("historical reference pages do not consume new-message capacity");
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn remote_admission_recovers_home_history_before_new_input(
	#[from(peer_mode)] mode: Arc<PeerMode>,
	#[from(peer_transform)]
	#[with(mode.clone())]
	transform: common::RouterTransform,
	#[future(awt)]
	#[from(common::native_peer)]
	#[with("aidash://execution-test",transform.clone())]
	home_fixture: common::PeerFixture,
	#[future(awt)]
	#[from(common::native_peer)]
	#[with("aidash://ordered-run-message-executor")]
	executor_fixture: common::PeerFixture,
) {
	let _ = &transform;
	let (home, home_url, home_schema) = home_fixture.runtime.parts();
	let (executor, executor_url, executor_schema) = executor_fixture.runtime.parts();
	let _ = &mode;
	let home_app = home_fixture.application;
	let executor_app = executor_fixture.application;
	bootstrap(&home, &home_app, "http://127.0.0.1:9").await;
	bootstrap(&executor, &executor_app, "http://127.0.0.1:9").await;
	add_peer(&home, &executor.config.node_id, "http://127.0.0.1:9").await;
	add_peer(&executor, &home.config.node_id, &home.config.endpoint).await;

	let workspace = home
		.store
		.create_workspace("Legacy run history", "Preserve correction order")
		.await
		.unwrap();
	let task = home
		.store
		.create_task(
			workspace.id,
			&NewTask {
				title: "Apply corrections".into(),
				description: "Respond to the latest instruction".into(),
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
	{
		let query_bind_1 = task.id;
		let query_bind_2 = &executor.config.node_id;
		let query_bind_3 = &agent.id;
		let query_bind_4 = &agent.version;
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
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(home.store.pool.driver())
		.await
	}
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
	delivered_history(
		&executor,
		&run,
		Uuid::new_v4(),
		"older correction from home history",
	)
	.await;
	let new_key = format!("human:{}:{}", run.id, Uuid::new_v4());
	executor
		.admit_run_message(
			&run,
			"human",
			"newer correction",
			&new_key,
			executor.run_message_limit(&run).await.unwrap(),
		)
		.await
		.unwrap();
	let inputs = executor.store.run_inputs(run.id).await.unwrap();
	let old_seq = inputs
		.iter()
		.find(|input| input.content == "older correction from home history")
		.unwrap()
		.seq;
	let new_seq = inputs
		.iter()
		.find(|input| input.content == "newer correction")
		.unwrap()
		.seq;
	assert!(old_seq < new_seq);
	drop(home_app);
	cleanup(home, &home_url, &home_schema).await;
	cleanup(executor, &executor_url, &executor_schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn remote_control_admits_before_delivery_and_rejects_late_side_effects(
	#[from(peer_mode)] mode: Arc<PeerMode>,
	#[from(peer_transform)]
	#[with(mode.clone())]
	transform: common::RouterTransform,
	#[future(awt)]
	#[from(common::native_peer)]
	#[with("aidash://execution-test",transform.clone())]
	home_fixture: common::PeerFixture,
	#[future(awt)]
	#[from(common::native_peer)]
	#[with("aidash://run-message-executor")]
	executor_fixture: common::PeerFixture,
) {
	let _ = &transform;
	let (home, home_url, home_schema) = home_fixture.runtime.parts();
	let (executor, executor_url, executor_schema) = executor_fixture.runtime.parts();
	let _ = &mode;
	let home_app = home_fixture.application;
	let executor_app = executor_fixture.application;
	bootstrap(&home, &home_app, "http://127.0.0.1:9").await;
	bootstrap(&executor, &executor_app, "http://127.0.0.1:9").await;
	add_peer(&home, &executor.config.node_id, "http://127.0.0.1:9").await;
	add_peer(&executor, &home.config.node_id, &home.config.endpoint).await;

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
	{
		let query_bind_1 = task.id;
		let query_bind_2 = &executor.config.node_id;
		let query_bind_3 = &agent.id;
		let query_bind_4 = &agent.version;
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
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(home.store.pool.driver())
		.await
	}
	.unwrap();
	let owner = qualified_agent(&executor.config.node_id, &agent.id, &agent.version);
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
	let first_key = Uuid::new_v4();
	delivered_history(&executor, &run, first_key, "remote correction").await;
	let mut old_lease = executor.store.pool.driver().begin().await.unwrap();
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
	{
		let query_bind_1 = run.id;
		let query_bind_2 = Uuid::new_v4();
		sqlx::query(
			&Query::update()
				.table(Alias::new("runs"))
				.value_expr(
					Alias::new("lease_owner"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.value_expr(
					Alias::new("lease_until"),
					Expr::cust("CURRENT_TIMESTAMP + INTERVAL '10 minutes'"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *old_lease)
		.await
	}
	.unwrap();
	old_lease.commit().await.unwrap();
	let observation = executor_app
		.raw_http
		.clone()
		.request(
			http::Method::GET,
			executor_app.url("/federation/v0.1/observe"),
		)
		.header(
			"authorization",
			format!(
				"Bearer {}",
				std::env::var("AIDASH_SECRET_TEST_PEER").unwrap()
			),
		)
		.header("x-aidash-node", &home.config.node_id)
		.header("x-aidash-protocol", "0.2")
		.send()
		.await
		.unwrap();
	assert_eq!(
		observation.status(),
		200,
		"peer observations must decode the full Run"
	);
	mode.commit_response_lost.store(true, Ordering::SeqCst);
	let first_admission = json!({
		"run_id":run.id,"action":"message","content":"remote correction","idempotency_key":first_key
	});
	let (status, body) =
		peer_control(&executor_app, &home.config.node_id, first_admission.clone()).await;
	assert_ne!(
		status, 200,
		"the simulated commit reply should be lost: {body}"
	);
	mode.commit_response_lost.store(false, Ordering::SeqCst);
	let pending_input = executor.store.run_inputs(run.id).await.unwrap().remove(0);
	assert!(
		pending_input.message_id.is_some(),
		"historical home delivery remains bound to the admitted input"
	);
	let (durable_after_admission, sequence_after_admission): (bool, Option<i64>) = {
		let query_bind_1 = task.id;
		let query_bind_2 = run.id;
		let query_bind_3 = format!("human:{}:{first_key}", run.id);
		sqlx::query_as(
			&Query::select()
				.expr(Expr::cust("expires_at IS NULL"))
				.column(Alias::new("input_seq"))
				.from(Alias::new("remote_run_message_fences"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
						Expr::value(query_bind_3.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(home.store.pool.driver())
		.await
	}
	.unwrap();
	assert!(durable_after_admission);
	assert_eq!(sequence_after_admission, Some(pending_input.seq));
	let (status, body) = peer_control(&executor_app, &home.config.node_id, first_admission).await;
	assert_eq!(
		status, 200,
		"exact retry recovers the admitted input: {body}"
	);
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
	{
		let query_bind_1 = task.id;
		let query_bind_2 = run.id;
		let query_bind_3 = &lease_key;
		sqlx::query(
			&Query::update()
				.table(Alias::new("remote_run_message_fences"))
				.value_expr(
					Alias::new("expires_at"),
					Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 second'"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
						Expr::value(query_bind_3.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(home.store.pool.driver())
		.await
	}
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
	{
		let query_bind_1 = run.id;
		sqlx::query(
			&Query::update()
				.table(Alias::new("runs"))
				.value_expr(Alias::new("lease_owner"), Expr::cust("NULL"))
				.value_expr(Alias::new("lease_until"), Expr::cust("NULL"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(executor.store.pool.driver())
		.await
	}
	.unwrap();
	executor
		.store
		.accept_run_message(
			run.id,
			"human",
			"lease renewal",
			&lease_key,
			executor.run_message_limit(&run).await.unwrap(),
		)
		.await
		.unwrap();
	remote_home
		.commit_run_message(&lease_key, "lease renewal")
		.await
		.unwrap();
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
	let retained_consumed_fence: Option<bool> = {
		let query_bind_1 = task.id;
		let query_bind_2 = run.id;
		let query_bind_3 = &lease_key;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("consumed"))
				.from(Alias::new("remote_run_message_fences"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
						Expr::value(query_bind_3.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(home.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(
		retained_consumed_fence,
		Some(false),
		"observing an input does not release the legacy-worker fence"
	);
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
	let released_count: i64 = {
		let query_bind_1 = task.id;
		let query_bind_2 = run.id;
		let query_bind_3 = &released_key;
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("remote_run_message_fences"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
						Expr::value(query_bind_3.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(home.store.pool.driver())
		.await
	}
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
	{
		let query_bind_1 = task.id;
		let query_bind_2 = terminal_history_run.id;
		let query_bind_3 = &terminal_history_key;
		sqlx::query(
			&Query::update()
				.table(Alias::new("remote_run_message_fences"))
				.value_expr(
					Alias::new("expires_at"),
					Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 second'"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
						Expr::value(query_bind_3.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(home.store.pool.driver())
		.await
	}
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
	assert!(matches!(
		Home::new(executor.clone(), response_run)
			.response_message(
				response_worker,
				0,
				&format!("{}:0:output", run.id),
				"stale response must not publish",
			)
			.await,
		Err(aidash_server::error::Error::StaleInference)
	));
	assert!(
		!home
			.store
			.snapshot(workspace.id)
			.await
			.unwrap()
			.messages
			.iter()
			.any(|message| message.content == "stale response must not publish")
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
		.transition(
			task.id,
			current_task.revision,
			&owner,
			aidash_server::domain::TaskStatus::Cancelled,
		)
		.await
		.expect_err("home termination must wait until the correction reaches inference");
	let response = error.http_response();
	assert_eq!(response.status, StatusCode::CONFLICT);
	assert_eq!(
		response
			.headers
			.get("x-aidash-run-message-pending")
			.and_then(|value| value.to_str().ok()),
		Some("1")
	);
	assert!(matches!(
		Home::new(executor.clone(), run.clone())
			.transition(aidash_server::domain::TaskStatus::Cancelled)
			.await,
		Err(aidash_server::error::Error::TransactionPending)
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
			&format!("{}:1:output", run.id),
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
	let still_fenced: bool = sqlx::query_scalar(
		"SELECT EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = $1 AND run_id = $2 AND NOT consumed)",
	)
	.bind(task.id)
	.bind(run.id)
	.fetch_one(home.store.pool.driver())
	.await
	.unwrap();
	assert!(
		still_fenced,
		"corrected output does not release old-worker fences"
	);
	assert!(
		remote_home
			.message(&format!("{}:0:output", run.id), "stale legacy output")
			.await
			.is_err(),
		"a legacy output path stays blocked after a corrected response is published"
	);
	executor
		.store
		.release_lease(run.id, response_worker)
		.await
		.unwrap();
	let legacy_key = Uuid::new_v4();
	delivered_history(&executor, &run, legacy_key, "recovered home history").await;
	executor.reconcile_run_messages(&run).await.unwrap();
	assert!(
		executor
			.store
			.run_inputs(run.id)
			.await
			.unwrap()
			.iter()
			.any(|input| input.content == "recovered home history")
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
	delivered_history(&executor, &run, large_key, &large_content).await;
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
		delivered_history(
			&executor,
			&run,
			Uuid::new_v4(),
			&format!("paged historical correction {index}"),
		)
		.await;
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
	// A peer without durable admission support must fail before any effect.
	mode.old_peer.store(true, Ordering::SeqCst);
	let unsupported_key = Uuid::new_v4();
	assert_eq!(peer_control(&executor_app, &home.config.node_id, json!({
		"run_id":run.id, "action":"message", "content":"unsupported peer correction", "idempotency_key":unsupported_key
	})).await.0, 409);
	assert!(
		!executor
			.store
			.run_inputs(run.id)
			.await
			.unwrap()
			.iter()
			.any(|input| input.content == "unsupported peer correction")
	);
	mode.old_peer.store(false, Ordering::SeqCst);
	// Re-delivery of an already committed message cannot alter its contents.
	home.store
		.message(
			workspace.id,
			&format!("human@{}", executor.config.node_id),
			"recovered home history",
			Some(&legacy_message_key),
		)
		.await
		.unwrap();
	assert!(
		home.store
			.message(
				workspace.id,
				&format!("human@{}", executor.config.node_id),
				"changed history",
				Some(&legacy_message_key)
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
	let admitted_fence_is_durable: bool = { let query_bind_1 = task.id; let query_bind_2 = run.id; let query_bind_3 = format!("human:{}:{pending_key}", run.id); sqlx::query_scalar(&Query::select()
			.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = ? AND run_id = ? AND idempotency_key = ? AND expires_at IS NULL AND NOT consumed))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()]))
			.to_string(PostgresQueryBuilder))
	.fetch_one(home.store.pool.driver())
	.await }
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
			.transition(
				task.id,
				current_task.revision,
				&owner,
				aidash_server::domain::TaskStatus::Cancelled
			)
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
	let observed_fence_still_active: bool = sqlx::query_scalar(
		"SELECT EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = $1 AND run_id = $2 AND NOT consumed)",
	)
	.bind(task.id)
	.bind(run.id)
	.fetch_one(home.store.pool.driver())
	.await
	.unwrap();
	assert!(
		observed_fence_still_active,
		"an inference acknowledgment cannot release a stale-worker fence"
	);
	// Recover the other run's delivered reservation after expiry. This run's
	// watermark must not consume that separate correction.
	terminal_history_home
		.reserve_run_message(&terminal_history_key, terminal_history_content)
		.await
		.unwrap();
	terminal_history_home
		.human_message_record(&terminal_history_key, terminal_history_content)
		.await
		.unwrap();
	assert!(
		home.store
			.transition(
				task.id,
				current_task.revision,
				&owner,
				aidash_server::domain::TaskStatus::Cancelled
			)
			.await
			.is_err(),
		"corrections for another run must still fence termination"
	);
	// The synthetic history run has no executor-side input ledger. Clear its
	// fixture-only fence after verifying it blocks termination above.
	{
		let query_bind_1 = task.id;
		let query_bind_2 = terminal_history_run.id;
		sqlx::query(
			&Query::update()
				.table(Alias::new("remote_run_message_fences"))
				.value_expr(Alias::new("consumed"), Expr::cust("TRUE"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ? AND run_id = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(home.store.pool.driver())
		.await
	}
	.unwrap();
	let completion_run = executor.store.run(run.id).await.unwrap();
	executor
		.deliver_run_messages(&completion_run)
		.await
		.unwrap();
	let through_seq = executor
		.store
		.run_inputs(run.id)
		.await
		.unwrap()
		.last()
		.map(|input| input.seq)
		.unwrap_or(0);
	let mut completion_run = executor.store.run(run.id).await.unwrap();
	completion_run.state = aidash_server::domain::RunState::ToolCall(Box::new(common::tool_call(
		json!({"included_input_seq":through_seq}),
	)));
	let artifact = ArtifactInput {
		kind: "text".into(),
		name: "Answer".into(),
		content: json!("Completed after observing corrections"),
	};
	assert!(
		home.store
			.complete(
				task.id,
				&owner,
				&format!("{}:legacy-complete", run.id),
				&artifact,
			)
			.await
			.is_err(),
		"the legacy completion path must remain blocked while run fences are active"
	);
	let completed = Home::new(executor.clone(), completion_run)
		.complete(&format!("{}:complete", run.id), &artifact)
		.await
		.unwrap();
	assert_eq!(
		completed.status,
		aidash_server::domain::TaskStatus::Completed
	);
	let remaining_current_fences: bool = sqlx::query_scalar(
		"SELECT EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = $1 AND run_id = $2 AND NOT consumed)",
	)
	.bind(task.id)
	.bind(run.id)
	.fetch_one(home.store.pool.driver())
	.await
	.unwrap();
	assert!(
		!remaining_current_fences,
		"terminal completion consumes covered fences atomically"
	);
	let rejected_after_home_terminal = Uuid::new_v4();
	assert_eq!(
		peer_control(
			&executor_app,
			&home.config.node_id,
			json!({
				"run_id":run.id,"action":"message","content":"after home completion","idempotency_key":rejected_after_home_terminal
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
			.any(|input| input.content == "after home completion")
	);
	let (status, body) = common::request(
		&executor_app,
		&executor.config.api_token,
		"POST",
		&format!("/api/runs/{}/message", run.id),
		json!({"content":"direct message after home completion","idempotency_key":Uuid::new_v4()}),
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
			.any(|input| input.content == "direct message after home completion")
	);
	{
		let query_bind_1 = run.id;
		sqlx::query(
			&Query::update()
				.table(Alias::new("runs"))
				.value_expr(Alias::new("phase"), Expr::cust("'CANCELLED'"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(executor.store.pool.driver())
		.await
	}
	.unwrap();
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
	{
		let query_bind_1 = run.id;
		sqlx::query(
			&Query::update()
				.table(Alias::new("runs"))
				.value_expr(
					Alias::new("pending"),
					Expr::value(common::pending(aidash_server::domain::RunState::Cancelled(
						aidash_server::domain::TerminalState {},
					))),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(executor.store.pool.driver())
		.await
	}
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
				"run_id":run.id,"action":"message","content":"recovered home history","idempotency_key":legacy_key
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
	{
		let query_bind_1 = expiry_task.id;
		sqlx::query(
			&Query::update()
				.table(Alias::new("remote_run_message_fences"))
				.value_expr(
					Alias::new("expires_at"),
					Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 second'"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(home.store.pool.driver())
		.await
	}
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
		.transition(
			expiry_task.id,
			expiry_task.revision,
			"human",
			aidash_server::domain::TaskStatus::Cancelled,
		)
		.await
		.unwrap();
	assert_eq!(
		cancelled.status,
		aidash_server::domain::TaskStatus::Cancelled
	);
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
	let historical_reservation_committed: bool = { let query_bind_1 = task.id; let query_bind_2 = terminal_history_run.id; let query_bind_3 = &terminal_history_key; sqlx::query_scalar(&Query::select()
			.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = ? AND run_id = ? AND idempotency_key = ? AND expires_at IS NULL))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()]))
			.to_string(PostgresQueryBuilder))
	.fetch_one(home.store.pool.driver())
	.await }
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
	let consumed_terminal_reservation: bool = {
		let query_bind_1 = task.id;
		let query_bind_2 = terminal_history_run.id;
		let query_bind_3 = &terminal_history_key;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("consumed"))
				.from(Alias::new("remote_run_message_fences"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
						Expr::value(query_bind_3.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(home.store.pool.driver())
		.await
	}
	.unwrap();
	assert!(consumed_terminal_reservation);
	let mut invalid_terminal_run = run.clone();
	invalid_terminal_run.id = Uuid::new_v4();
	let invalid_terminal_key = format!("human:{}:{}", invalid_terminal_run.id, Uuid::new_v4());
	sqlx::query("ALTER TABLE tasks DISABLE TRIGGER gate_remote_task_terminal")
		.execute(home.store.pool.driver())
		.await
		.unwrap();
	{
		let query_bind_1 = task.id;
		sqlx::query(
			&Query::update()
				.table(Alias::new("tasks"))
				.value_expr(Alias::new("status"), Expr::cust("'RUNNING'"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(home.store.pool.driver())
		.await
	}
	.unwrap();
	sqlx::query("ALTER TABLE tasks ENABLE TRIGGER gate_remote_task_terminal")
		.execute(home.store.pool.driver())
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
		.execute(home.store.pool.driver())
		.await
		.unwrap();
	{
		let query_bind_1 = task.id;
		sqlx::query(
			&Query::update()
				.table(Alias::new("tasks"))
				.value_expr(Alias::new("status"), Expr::cust("'CANCELLED'"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(home.store.pool.driver())
		.await
	}
	.unwrap();
	sqlx::query("ALTER TABLE tasks ENABLE TRIGGER gate_remote_task_terminal")
		.execute(home.store.pool.driver())
		.await
		.unwrap();
	{
		let query_bind_1 = task.id;
		let query_bind_2 = terminal_history_run.id;
		let query_bind_3 = &terminal_history_key;
		sqlx::query(
			&Query::update()
				.table(Alias::new("remote_run_message_fences"))
				.value_expr(Alias::new("consumed"), Expr::cust("FALSE"))
				.value_expr(
					Alias::new("expires_at"),
					Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 second'"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
						Expr::value(query_bind_3.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(home.store.pool.driver())
		.await
	}
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
	let recovered_terminal_reservation: bool = { let query_bind_1 = task.id; let query_bind_2 = terminal_history_run.id; let query_bind_3 = &terminal_history_key; sqlx::query_scalar(&Query::select()
			.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = ? AND run_id = ? AND idempotency_key = ? AND expires_at IS NULL AND NOT consumed))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()]))
			.to_string(PostgresQueryBuilder))
	.fetch_one(home.store.pool.driver())
	.await }
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
	drop(home_app);
	cleanup(home, &home_url, &home_schema).await;
	cleanup(executor, &executor_url, &executor_schema).await;
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;

#[rstest::fixture]
fn peer_mode() -> Arc<PeerMode> {
	Arc::new(PeerMode {
		old_peer: AtomicBool::new(false),
		delivery_outage: AtomicBool::new(false),
		commit_response_lost: AtomicBool::new(false),
	})
}
#[rstest::fixture]
fn peer_transform(peer_mode: Arc<PeerMode>) -> common::RouterTransform {
	Arc::new(move |router| router.with_middleware(PeerFaults(peer_mode.clone())))
}
