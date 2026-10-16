//! Web disclosure approval through the real Harness, worker authority and
//! approval HTTP API: a scripted model calls the research fixture package,
//! whose disclosure waits for an exact approval that one dispatch consumes.
use super::*;
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_server::{
	domain::{Run, RunPhase, RunState, WaitingState},
	harness::Harness,
};
use reinhardt::ServerRouter as Router;
use std::sync::atomic::{AtomicUsize, Ordering};
use upstream_fixtures::reply;

const VERSION: &str = "3.0.0";
const QUERY: &str = "strasse line";

struct Research {
	c: CoreFixture,
	_provider: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	requests: Arc<AtomicUsize>,
}

#[rstest::fixture]
fn research_requests() -> Arc<AtomicUsize> {
	Arc::new(AtomicUsize::new(0))
}
/// Every model request calls `research_search` with the same query.
#[rstest::fixture]
fn research_router(#[from(research_requests)] requests: Arc<AtomicUsize>) -> Arc<Router> {
	let provider = reinhardt::test::stub::StubRouter::new()
		.route(
			"/v1/chat/completions",
			http::Method::POST,
			reply(move |_: reinhardt::Request| {
				let counter = requests.clone();
				async move {
					let count = counter.fetch_add(1, Ordering::SeqCst) + 1;
					reinhardt::Response::ok()
						.with_json(&json!({
							"choices":[{"index":0,"finish_reason":"tool_calls","message":{
								"role":"assistant","content":null,
								"tool_calls":[{"id":format!("search-{count}"),"type":"function","function":{
									"name":"research_search","arguments":json!({"query":QUERY}).to_string()
								}}]
							}}],
							"usage":{"prompt_tokens":1,"completion_tokens":1}
						}))
						.unwrap()
				}
			}),
		)
		.into_server_router();
	Arc::new(provider)
}

#[rstest::fixture]
async fn research_fixture(
	#[from(research_requests)] requests: Arc<AtomicUsize>,
	#[from(research_router)]
	#[with(requests.clone())]
	_router: Arc<Router>,
	#[from(upstream_fixtures::provider_transport)]
	#[with(_router.clone())]
	provider: upstream_fixtures::UpstreamFuture,
	#[from(provider_endpoint)]
	#[with(provider.clone())]
	_endpoint: EndpointFuture,
	#[from(capability_fixture)]
	#[with("aidash://research",_endpoint.clone())]
	core: CoreFuture,
) -> Research {
	let mut c = core.await;
	c.policy["subjects"]
		[aidash_server::domain::qualified_agent(&c.f.config.node_id, "research", VERSION)] =
		json!({"kind":"agent"});
	let (status, value) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":c.policy}),
	)
	.await;
	assert_eq!(status, 200, "{value}");
	// Install, review and activate the fixture research package like any other.
	let (status, pending) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/marketplace/host-packages",
		json!({"tenant":"acme","groups":["research_fixture"],"idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 200, "{pending}");
	let installations = pending["installations"].as_array().unwrap().clone();
	assert_eq!(installations.len(), 4, "{pending}");
	let selection = json!({"tenant":"acme","installations":installations.iter().map(|r|json!({"installation":r["installation"]["id"],"revision":r["revision"],"digest":r["digest"],"expected_activation_revision":0})).collect::<Vec<_>>(),"approvals":installations.iter().map(|r|json!({"reference":{"id":r["entry"]["id"],"version":r["entry"]["version"]},"expected_catalog_revision":0})).collect::<Vec<_>>()});
	let (status, activated) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/marketplace/approval-sets",
		selection,
	)
	.await;
	assert_eq!(status, 200, "{activated}");
	let bundle = installations
		.iter()
		.find(|row| row["entry"]["kind"] == "bundle")
		.unwrap();
	let mut agent = c.f.registry.get("research", "1.1.0").await.unwrap();
	agent.version = VERSION.into();
	agent.binding_normalization = None;
	agent.config["instructions"] = json!("Research with the bound tools.");
	agent.config["bindings"] = json!([{"kind":"bundle","target":{"registry_node":c.f.config.node_id,"id":bundle["entry"]["id"],"version":bundle["entry"]["version"]},"narrow":{}}]);
	agent.config["remove_default"] = json!([]);
	for (method, path, body) in [
		("POST", "/api/registry".to_owned(), json!(agent)),
		(
			"POST",
			"/api/authorization/acme/catalog".to_owned(),
			json!({"entry":{"id":"research","version":VERSION},"expected_revision":0,"enabled":true}),
		),
	] {
		let (status, value) = request(&c.app, &c.f.config.api_token, method, &path, body).await;
		assert_eq!(status, 200, "{value}");
	}
	Research {
		c,
		_provider: provider.await,
		requests,
	}
}

impl Research {
	/// After admission, requests and approvals stay allowed on research
	/// resources, but no policy permits `research.disclose`, so every
	/// disclosure needs an approval.
	async fn require_disclosure_approval(&mut self) {
		let policy = &mut self.c.policy;
		policy["policies"][0]["resources"]["kinds"] = json!([
			"workspace",
			"task",
			"run",
			"tool",
			"installation",
			"registry_entry",
			"message",
			"agent",
			"model",
			"skill",
			"memory",
			"working_area",
			"artifact",
			"human_request",
			"bundle"
		]);
		policy["policies"].as_array_mut().unwrap().push(json!({"id":"approvable-research","effect":"allow","subjects":{"any":true},"actions":["capability.request","capability.approve"],"resources":{"kinds":["research"]}}));
		let (status, value) = request(
			&self.c.app,
			&self.c.f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":3,"bundle":self.c.policy}),
		)
		.await;
		assert_eq!(status, 200, "{value}");
	}
	async fn delegate(&self) -> Run {
		let (status, value) = request(
			&self.c.app,
			&self.c.token,
			"POST",
			&format!("/api/tasks/{}/delegate", self.c.task),
			json!({"node_id":self.c.f.config.node_id,"agent":{"id":"research","version":VERSION}}),
		)
		.await;
		assert_eq!(status, 200, "{value}");
		self.c.f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0)
	}
	/// Step the real worker until the Run waits on a Web disclosure approval.
	async fn until_waiting(&self, id: Uuid) -> Uuid {
		let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
		loop {
			Harness {
				federation: self.c.f.clone(),
			}
			.worker_once()
			.await
			.unwrap();
			let run = self.c.f.store.run(&TEST_FIXTURE, id).await.unwrap();
			if let RunState::Waiting(waiting) = &run.state
				&& let WaitingState::CoreApproval { approval_id, .. } = waiting.as_ref()
			{
				return *approval_id;
			}
			if run.error.is_some() || tokio::time::Instant::now() >= deadline {
				use reinhardt::query::{Alias, Expr, Order, PostgresQueryBuilder, Query};
				let sql = Query::select()
					.expr(Expr::cust(
						"subject || ' ' || action || ' ' || resource_kind || ':' || resource_id",
					))
					.from(Alias::new("authorization_decisions"))
					.and_where(Expr::cust("(decision->>'allowed')::boolean IS NOT TRUE"))
					.order_by(Alias::new("sequence"), Order::Desc)
					.limit(5)
					.to_string(PostgresQueryBuilder);
				let denied: Vec<String> = sqlx::query_scalar(&sql)
					.fetch_all(self.c.f.store.pool.driver())
					.await
					.unwrap_or_default();
				panic!("run failed: error={:?} denied={denied:?}", run.error);
			}
			assert!(
				!matches!(
					run.phase(),
					RunPhase::Completed | RunPhase::Failed | RunPhase::Cancelled
				) && tokio::time::Instant::now() < deadline,
				"run did not wait for approval: phase={} error={:?} state={:?}",
				run.phase(),
				run.error,
				run.state
			);
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	}
	async fn attempts(&self, run: Uuid) -> i64 {
		use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
		let sql = Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("research_attempts"))
			.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run)))
			.to_string(PostgresQueryBuilder);
		sqlx::query_scalar(&sql)
			.fetch_one(self.c.f.store.pool.driver())
			.await
			.unwrap()
	}
	async fn close(self) {
		drop(self._provider);
		self.c.close().await;
	}
}

#[rstest::rstest]
#[tokio::test]
async fn web_disclosure_waits_for_an_exact_approval_that_one_dispatch_consumes(
	#[future] research_fixture: Research,
) {
	// Arrange
	let mut r = research_fixture.await;
	let run = r.delegate().await;
	r.require_disclosure_approval().await;

	// Act: the first search waits for a Web disclosure approval.
	let first = r.until_waiting(run.id).await;
	assert_eq!(
		r.attempts(run.id).await,
		0,
		"nothing is sent before approval"
	);
	let (status, approvals) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		"/api/capabilities/approvals",
		json!(null),
	)
	.await;
	assert_eq!(status, 200, "{approvals}");
	let card = approvals["items"]
		.as_array()
		.unwrap()
		.iter()
		.find(|item| item["id"] == json!(first))
		.unwrap_or_else(|| panic!("{approvals}"));
	assert_eq!(card["kind"], "disclosure");
	assert_eq!(card["action"], "research.disclose");
	assert_eq!(card["state"], "pending");
	assert_eq!(
		card["disclosure"], QUERY,
		"the approver sees the exact query"
	);
	assert_eq!(card["targets"], json!(["research-fixture.invalid"]));
	let (status, decided) = request(
		&r.c.app,
		&r.c.token,
		"POST",
		&format!("/api/capabilities/approvals/{first}/decide"),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":card["revision"]}),
	)
	.await;
	assert_eq!(status, 200, "{decided}");
	assert_eq!(decided["state"], "approved");

	// The resumed Run repeats the call: exactly one dispatch consumes the
	// approval, and the following identical call waits for a new one.
	let second = r.until_waiting(run.id).await;
	assert_ne!(second, first);
	assert_eq!(r.attempts(run.id).await, 1, "exactly one dispatch");
	let run = r.c.f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	let events = run.context.events().collect::<Vec<_>>();
	let statuses = serde_json::to_value(events)
		.unwrap()
		.as_array()
		.unwrap()
		.iter()
		.filter(|event| event["call"]["name"] == "research_search")
		.map(|event| {
			event["result"]["status"]
				.as_str()
				.unwrap_or_default()
				.to_owned()
		})
		.collect::<Vec<_>>();
	assert_eq!(statuses, ["approval_required", "ok", "approval_required"]);
	let (_, approvals) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		"/api/capabilities/approvals",
		json!(null),
	)
	.await;
	let states = approvals["items"]
		.as_array()
		.unwrap()
		.iter()
		.filter(|item| item["kind"] == "disclosure")
		.map(|item| {
			(
				item["id"].as_str().unwrap().to_owned(),
				item["state"].as_str().unwrap().to_owned(),
			)
		})
		.collect::<std::collections::BTreeMap<_, _>>();
	assert_eq!(states[&first.to_string()], "consumed");
	assert_eq!(states[&second.to_string()], "pending");
	assert_eq!(r.requests.load(Ordering::SeqCst), 3);
	r.close().await;
}

const PAGE: &str = "https://research-fixture.example/strasse";

/// The first model request reads the fixture page's first line; once a
/// delivered fragment is in the conversation, the answer cites it.
fn citing_router() -> Arc<Router> {
	let provider = reinhardt::test::stub::StubRouter::new()
		.route(
			"/v1/chat/completions",
			http::Method::POST,
			reply(move |request: reinhardt::Request| {
				async move {
					// Only a delivered fragment puts an `ev_<uuid>` into the request.
					let body = request.json::<Value>().unwrap().to_string();
					let delivered = evidence_ref(&body);
					let (finish, message) = match delivered {
						Some(reference) => (
							"stop",
							json!({"role":"assistant","content":format!("The page opens with Alpha [[cite:{reference}]].")}),
						),
						None => (
							"tool_calls",
							json!({"role":"assistant","content":null,"tool_calls":[{"id":"fetch-1","type":"function","function":{
								"name":"research_fetch","arguments":json!({"url":PAGE,"max_bytes":5}).to_string()
							}}]}),
						),
					};
					reinhardt::Response::ok()
						.with_json(&json!({
							"choices":[{"index":0,"finish_reason":finish,"message":message}],
							"usage":{"prompt_tokens":1,"completion_tokens":1}
						}))
						.unwrap()
				}
			}),
		)
		.into_server_router();
	Arc::new(provider)
}

/// The first `ev_<uuid>` in a tool result, as the Agent would cite it.
fn evidence_ref(content: &str) -> Option<String> {
	content.match_indices("ev_").find_map(|(start, _)| {
		let candidate = content.get(start..start + 39)?;
		Uuid::parse_str(&candidate[3..]).ok()?;
		Some(candidate.to_owned())
	})
}

impl Research {
	/// Step the real worker until the Run completes.
	async fn until_completed(&self, id: Uuid) -> Run {
		let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
		loop {
			Harness {
				federation: self.c.f.clone(),
			}
			.worker_once()
			.await
			.unwrap();
			let run = self.c.f.store.run(&TEST_FIXTURE, id).await.unwrap();
			if run.phase() == RunPhase::Completed {
				return run;
			}
			if matches!(run.phase(), RunPhase::Failed | RunPhase::Cancelled)
				|| tokio::time::Instant::now() >= deadline
			{
				let results = serde_json::to_value(run.context.events().collect::<Vec<_>>())
					.unwrap()
					.as_array()
					.unwrap()
					.iter()
					.filter_map(|event| event.get("result").cloned())
					.take(3)
					.collect::<Vec<_>>();
				panic!(
					"run did not complete: phase={} error={:?} results={results:?}",
					run.phase(),
					run.error
				);
			}
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	}
	/// The contents of the Run's own messages or Artifacts.
	async fn published(&self, table: &str, run: Uuid) -> Vec<Value> {
		use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
		let sql = Query::select()
			.expr(Expr::cust(if table == "messages" {
				"to_jsonb(content)"
			} else {
				"content"
			}))
			.from(Alias::new(table))
			.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run)))
			.to_string(PostgresQueryBuilder);
		sqlx::query_scalar(&sql)
			.fetch_all(self.c.f.store.pool.driver())
			.await
			.unwrap()
	}
}

#[rstest::rstest]
#[tokio::test]
async fn a_reader_of_the_run_opens_its_cited_fragment_and_everyone_else_sees_the_same_absence(
	#[future]
	#[with(Default::default(), citing_router())]
	research_fixture: Research,
) {
	// Arrange: the Run reads the page and publishes an answer citing it.
	let mut r = research_fixture.await;
	let run = r.delegate().await;
	r.until_completed(run.id).await;
	let messages = r.published("messages", run.id).await;
	let answer = messages
		.iter()
		.filter_map(Value::as_str)
		.find(|content| content.contains("[[cite:"))
		.unwrap_or_else(|| panic!("{messages:?}"));
	let reference = evidence_ref(answer).unwrap();
	assert_eq!(
		r.published("artifacts", run.id).await,
		[json!(answer)],
		"the Task's text result records its publishing Run"
	);
	let path = format!("/api/runs/{}/evidence/{reference}", run.id);

	// Act
	let (status, evidence) = request(&r.c.app, &r.c.token, "GET", &path, json!(null)).await;

	// Assert: the fragment as delivered, its snapshot and the undelivered context.
	assert_eq!(status, 200, "{evidence}");
	assert_eq!(evidence["evidence_ref"], reference);
	assert_eq!(evidence["run_id"], json!(run.id));
	assert_eq!(evidence["text"], "Alpha");
	assert_eq!(
		(&evidence["line_start"], &evidence["line_end"]),
		(&json!(1), &json!(1))
	);
	assert_eq!(evidence["url"], PAGE);
	assert_eq!(evidence["requested_url"], PAGE);
	assert_eq!(evidence["title"], "Die Straße");
	assert_eq!(evidence["completeness"], "complete");
	assert!(evidence["fetched_at"].is_string());
	assert_eq!(evidence["context"]["before"], json!([]));
	assert_eq!(
		evidence["context"]["after"],
		json!([
			{"text":"Die Straße <script>inert text</script>","page":2},
			{"text":"Omega line","page":2}
		])
	);
	let (status, deployment_administrator) =
		request(&r.c.app, &r.c.f.config.api_token, "GET", &path, json!(null)).await;
	assert_eq!(
		status, 200,
		"a Deployment Administrator reads what run_get discloses"
	);
	assert_eq!(deployment_administrator["text"], "Alpha");

	// Everyone else gets one identical absence, whatever the reason. Bob is
	// a member of the Tenant whose policy no longer lets him read Runs.
	r.c.policy["policies"].as_array_mut().unwrap().push(json!({"id":"bob-reads-no-runs","effect":"deny","subjects":{"ids":["bob"]},"actions":["run.read"],"resources":{"kinds":["run"]}}));
	let (status, value) = request(
		&r.c.app,
		&r.c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":3,"bundle":r.c.policy}),
	)
	.await;
	assert_eq!(status, 200, "{value}");
	let (_, bob) = request(
		&r.c.app,
		&r.c.f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	let unknown = format!("ev_{}", Uuid::new_v4());
	for (case, token, path) in [
		(
			"a subject who cannot read the Run",
			bob["token"].as_str().unwrap().to_owned(),
			path.clone(),
		),
		(
			"a reference the Run does not hold",
			r.c.token.clone(),
			format!("/api/runs/{}/evidence/{unknown}", run.id),
		),
		(
			"a malformed reference",
			r.c.token.clone(),
			format!("/api/runs/{}/evidence/ev_nope", run.id),
		),
		(
			"another Run",
			r.c.token.clone(),
			format!("/api/runs/{}/evidence/{reference}", Uuid::new_v4()),
		),
	] {
		let (status, body) = request(&r.c.app, &token, "GET", &path, json!(null)).await;
		assert_eq!(status, 404, "{case}: {body}");
		assert_eq!(body, json!({"error":"evidence unavailable"}), "{case}");
	}
	r.close().await;
}
