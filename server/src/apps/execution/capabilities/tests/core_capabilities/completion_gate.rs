//! The Completion Gate (#112) through the real Harness, scheduler and this
//! node's adapters. A scripted provider answers without tool calls, so every
//! answer is a Completion Candidate. This profile has no runner, so its shell
//! Binding never resolves and no Check can start here: such a candidate pauses
//! for a Completion Review. Checks that do run are covered by the harness and
//! the runner suites.
use super::*;
use aidash_server::{
	domain::{Run, RunPhase},
	harness::Harness,
};
use reinhardt::ServerRouter as Router;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use std::sync::Mutex;
use upstream_fixtures::reply;

const VERSION: &str = "3.0.0";
const CLAIM: &str = "All tests pass ✅";

/// Every request the provider received, in order.
type Requests = Arc<Mutex<Vec<Value>>>;

struct Gate {
	c: CoreFixture,
	requests: Requests,
	_provider: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
}

#[rstest::fixture]
fn gate_requests() -> Requests {
	Arc::default()
}

/// The provider never calls a tool: every answer is a Completion Candidate.
#[rstest::fixture]
fn gate_provider_router(#[from(gate_requests)] requests: Requests) -> Arc<Router> {
	let provider = reinhardt::test::stub::StubRouter::new()
		.route(
			"/v1/chat/completions",
			http::Method::POST,
			reply(move |request: reinhardt::Request| {
				let count = {
					let mut requests = requests.lock().unwrap();
					requests.push(request.json::<Value>().unwrap());
					requests.len()
				};
				async move {
					let content = if count == 1 {
						CLAIM.to_owned()
					} else {
						format!("Answer {count}")
					};
					reinhardt::Response::ok()
						.with_json(&json!({"choices":[{"index":0,"finish_reason":"stop",
							"message":{"role":"assistant","content":content}}],
							"usage":{"prompt_tokens":1,"completion_tokens":1}}))
						.unwrap()
				}
			}),
		)
		.into_server_router();
	Arc::new(provider)
}

#[rstest::fixture]
async fn gate_fixture(
	#[from(gate_requests)] requests: Requests,
	#[from(gate_provider_router)]
	#[with(requests.clone())]
	_router: Arc<Router>,
	#[from(upstream_fixtures::provider_transport)]
	#[with(_router.clone())]
	provider: upstream_fixtures::UpstreamFuture,
	#[from(provider_endpoint)]
	#[with(provider.clone())]
	_endpoint: EndpointFuture,
	#[from(capability_fixture)]
	#[with("aidash://completion",_endpoint.clone())]
	core: CoreFuture,
) -> Gate {
	let mut c = core.await;
	// The fixture's research Agent plus a Completion Policy the model never sees.
	let mut agent = c.f.registry.get("research", "1.1.0").await.unwrap();
	agent.version = VERSION.into();
	agent.binding_normalization = None;
	agent.config["instructions"] = json!("Answer the task. Completion Checks verify it.");
	agent.config["completion"] = json!({"version":"checks/1","checks":[{"name":"tests",
		"capability":"shell","command":"cargo test --offline","when":"always"}]});
	let (status, result) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/registry",
		json!(agent),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	let (status, result) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"research","version":VERSION},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	c.policy["subjects"]
		[aidash_server::domain::qualified_agent(&c.f.config.node_id, "research", VERSION)] =
		json!({"kind":"agent"});
	let (status, result) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":c.policy}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	Gate {
		c,
		requests,
		_provider: provider.await,
	}
}

impl Gate {
	async fn delegate(&self) -> Run {
		let (status, result) = request(
			&self.c.app,
			&self.c.token,
			"POST",
			&format!("/api/tasks/{}/delegate", self.c.task),
			json!({"node_id":self.c.f.config.node_id,"agent":{"id":"research","version":VERSION}}),
		)
		.await;
		assert_eq!(status, 200, "{result}");
		self.c
			.f
			.store
			.runs(&TEST_FIXTURE)
			.await
			.unwrap()
			.into_iter()
			.find(|run| run.task_id == self.c.task)
			.unwrap()
	}
	/// Step on a fresh Harness, through the scheduler's own lease, until `done`
	/// holds for the stored Run.
	async fn drive(&self, id: Uuid, done: impl Fn(&Run) -> bool) -> Run {
		let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
		loop {
			Harness {
				federation: self.c.f.clone(),
			}
			.worker_once()
			.await
			.unwrap();
			let run = self.c.f.store.run(&TEST_FIXTURE, id).await.unwrap();
			if done(&run) {
				return run;
			}
			assert!(
				!matches!(
					run.phase(),
					RunPhase::Completed | RunPhase::Failed | RunPhase::Cancelled
				),
				"error={:?} state={:?}",
				run.error,
				run.state
			);
			assert!(
				tokio::time::Instant::now() < deadline,
				"run stalled: phase={} state={:?} completion={:?}",
				run.phase(),
				run.state,
				run.context.completion
			);
			tokio::time::sleep(std::time::Duration::from_millis(100)).await;
		}
	}
	/// `(prompt, response, answered_by)` of a human request.
	async fn human_request(&self, id: Uuid) -> (String, Option<Value>, Option<String>) {
		sqlx::query_as(
			&Query::select()
				.columns(["prompt", "response", "answered_by"].map(Alias::new))
				.from(Alias::new("human_requests"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(self.c.f.store.pool.driver())
		.await
		.unwrap()
	}
	async fn close(self) {
		drop(self._provider);
		self.c.close().await;
	}
}

/// AC 4 of #112 on a node whose Checks cannot run: the model's claim pauses
/// for a Completion Review instead of completing the Task. A new message
/// wakes the waiting Run, which answers the stale review itself, and the
/// model's next answer is reviewed again.
#[rstest::rstest]
#[tokio::test]
async fn new_input_supersedes_a_review_of_a_check_this_node_cannot_run(
	#[future] gate_fixture: Gate,
) {
	// Arrange
	let g = gate_fixture.await;
	let run = g.delegate().await;
	let run = g
		.drive(run.id, |run| run.context.completion.review.is_some())
		.await;
	let first = run.context.completion.review.unwrap();
	let (prompt, response, _) = g.human_request(first).await;
	assert!(
		prompt.starts_with("Completion Review: Check tests could not run"),
		"{prompt}"
	);
	assert_eq!(response, None);
	assert_eq!(run.context.completion.attempts, 0, "no Check started");
	assert_eq!(run.context.completion.reviews, 1);
	// Act
	let (status, body) = request(
		&g.c.app,
		&g.c.token,
		"POST",
		&format!("/api/runs/{}/message", run.id),
		json!({"content":"Use the fixture data instead","idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let run = g
		.drive(run.id, |run| {
			run.context
				.completion
				.review
				.is_some_and(|review| review != first)
		})
		.await;
	// Assert: the stale review was answered on the Run's behalf, and the new
	// candidate went to a person again rather than completing the Task.
	let (_, response, answered_by) = g.human_request(first).await;
	assert_eq!(response, Some(json!({"superseded":true})));
	assert_eq!(answered_by.as_deref(), Some("system"));
	assert_eq!(run.context.completion.reviews, 2);
	assert_ne!(run.phase(), RunPhase::Completed);
	let requests = g.requests.lock().unwrap().clone();
	assert_eq!(requests.len(), 2, "the new input formed a new candidate");
	assert!(
		requests[1]
			.to_string()
			.contains("Use the fixture data instead"),
		"{}",
		requests[1]
	);
	g.close().await;
}
