//! Web disclosure approval through the real Harness, worker authority and
//! approval HTTP API: a scripted model calls the research fixture package,
//! whose disclosure waits for an exact approval that one dispatch consumes.
use super::*;
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
		self.c.f.store.runs().await.unwrap().remove(0)
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
			let run = self.c.f.store.run(id).await.unwrap();
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
	let run = r.c.f.store.run(run.id).await.unwrap();
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
