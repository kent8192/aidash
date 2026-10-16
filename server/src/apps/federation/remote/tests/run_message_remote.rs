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
