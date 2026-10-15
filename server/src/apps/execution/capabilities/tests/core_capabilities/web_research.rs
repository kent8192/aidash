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
/// The first page a URL intent asks the Run to read, found in any text of a
/// model request, including JSON-encoded context.
fn intent_page(value: &Value) -> Option<String> {
	use aidash_domain::research::intent::URL_INTENT_HEADING;
	match value {
		Value::String(text) if text.contains(URL_INTENT_HEADING) => {
			serde_json::from_str::<Value>(text)
				.ok()
				.and_then(|inner| intent_page(&inner))
				.or_else(|| {
					let after = &text[text.find(URL_INTENT_HEADING)? + URL_INTENT_HEADING.len()..];
					let start = after.find("\n- ")? + 3;
					Some(after[start..].split(['\n', '"']).next()?.to_owned())
				})
		}
		Value::Array(items) => items.iter().find_map(intent_page),
		Value::Object(fields) => fields.values().find_map(intent_page),
		_ => None,
	}
}

/// Every model request calls `research_search` with the same query, unless
/// the Run was asked to read a page through a URL intent: then it reads the
/// first page listed.
#[rstest::fixture]
fn research_router(#[from(research_requests)] requests: Arc<AtomicUsize>) -> Arc<Router> {
	let provider = reinhardt::test::stub::StubRouter::new()
		.route(
			"/v1/chat/completions",
			http::Method::POST,
			reply(move |request: reinhardt::Request| {
				let counter = requests.clone();
				let page = request
					.json::<Value>()
					.ok()
					.and_then(|body| intent_page(&body));
				async move {
					let count = counter.fetch_add(1, Ordering::SeqCst) + 1;
					let (name, arguments) = match page {
						Some(url) => ("research_fetch", json!({"url":url})),
						None => ("research_search", json!({"query":QUERY})),
					};
					reinhardt::Response::ok()
						.with_json(&json!({
							"choices":[{"index":0,"finish_reason":"tool_calls","message":{
								"role":"assistant","content":null,
								"tool_calls":[{"id":format!("call-{count}"),"type":"function","function":{
									"name":name,"arguments":arguments.to_string()
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

#[rstest::rstest]
#[tokio::test]
async fn a_url_intent_asks_for_a_page_reading_whose_disclosure_still_waits_for_approval(
	#[future] research_fixture: Research,
) {
	// Arrange: a Thread whose next Run is asked to read one pasted page.
	let mut r = research_fixture.await;
	r.require_disclosure_approval().await;
	let workspace = r.c.f.store.task(r.c.task).await.unwrap().workspace_id;
	let root = source(&r.c, workspace, "Read the release notes").await;
	let (status, thread) = request(
		&r.c.app,
		&r.c.token,
		"POST",
		&format!("/api/workspaces/{workspace}/threads"),
		json!({"root_message_id":root}),
	)
	.await;
	assert_eq!(status, 200, "{thread}");
	let path = format!(
		"/api/workspaces/{workspace}/threads/{}/agents/research/runs",
		thread["id"].as_str().unwrap()
	);
	let page = "https://example.com/notes";

	// Act: an address the host could not read is refused before any work.
	let (status, refused) = request(
		&r.c.app,
		&r.c.token,
		"POST",
		&path,
		json!({"idempotency_key":Uuid::new_v4(),"agent_version":VERSION,"title":"Notes","description":"Summarize them.","read_pages":["https://user:secret@example.com/"]}),
	)
	.await;
	assert_eq!(status, 400, "{refused}");
	assert!(
		refused.to_string().contains("INVALID_URL_INTENT"),
		"{refused}"
	);
	let (status, area) = request(
		&r.c.app,
		&r.c.token,
		"POST",
		&path,
		json!({"idempotency_key":Uuid::new_v4(),"agent_version":VERSION,"title":"Notes","description":"Summarize them.","read_pages":[page]}),
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let (_, session) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		&format!(
			"/api/working-areas/{}/session",
			area["id"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	let run: Uuid = serde_json::from_value(session["queue"][0]["run_id"].clone())
		.unwrap_or_else(|_| panic!("{session}"));
	let approval = r.until_waiting(run).await;

	// Assert: the host rendered the request; reading the page still waits for
	// its own Web disclosure approval, filed in the Run's Thread.
	let task =
		r.c.f
			.store
			.task(r.c.f.store.run(run).await.unwrap().task_id)
			.await
			.unwrap();
	assert_eq!(
		task.description,
		format!(
			"Summarize them.\n\n{}\n- {page}",
			aidash_domain::research::intent::URL_INTENT_HEADING
		)
	);
	assert_eq!(r.attempts(run).await, 0, "nothing is sent before approval");
	let (status, inbox) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		"/api/capabilities/approvals?state=pending&role=approver",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{inbox}");
	let card = inbox["items"]
		.as_array()
		.unwrap()
		.iter()
		.find(|item| item["id"] == json!(approval))
		.unwrap_or_else(|| panic!("{inbox}"));
	assert_eq!(card["kind"], "disclosure");
	assert_eq!(card["state"], "pending");
	assert_eq!(card["operation"], "fetch");
	assert_eq!(card["provider"], "research.fixture@1");
	assert_eq!(card["disclosure"], page);
	assert_eq!(card["targets"], json!(["example.com"]));
	assert_eq!(card["area_id"], area["id"]);
	assert_eq!(card["viewer_is_approver"], true);
	assert!(card.get("operator_account").is_none(), "{card}");
	r.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn the_run_research_api_reports_usage_with_run_get_authority_and_the_inbox_lists_only_what_awaits(
	#[future] research_fixture: Research,
) {
	// Arrange: one search approved for this Run, which then waits again.
	let mut r = research_fixture.await;
	let run = r.delegate().await;
	r.require_disclosure_approval().await;
	let first = r.until_waiting(run.id).await;
	let (_, inbox) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		"/api/capabilities/approvals?state=pending&role=approver",
		Value::Null,
	)
	.await;
	let card = inbox["items"]
		.as_array()
		.unwrap()
		.iter()
		.find(|item| item["id"] == json!(first))
		.unwrap_or_else(|| panic!("{inbox}"))
		.clone();
	assert_eq!(card["operation"], "search");
	assert_eq!(card["query"]["query"], QUERY);
	assert_eq!(card["agent_id"], "research");
	assert_eq!(card["agent_version"], VERSION);
	let (status, decided) = request(
		&r.c.app,
		&r.c.token,
		"POST",
		&format!("/api/capabilities/approvals/{first}/decide"),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":card["revision"],"choice":"allow_run","targets":card["targets"],"expires_at":chrono::Utc::now() + chrono::Duration::minutes(10)}),
	)
	.await;
	assert_eq!(status, 200, "{decided}");
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
	while r.attempts(run.id).await == 0 {
		assert!(
			tokio::time::Instant::now() < deadline,
			"the search never dispatched"
		);
		Harness {
			federation: r.c.f.clone(),
		}
		.worker_once()
		.await
		.unwrap();
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
	}

	// Act
	let (status, research) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		&format!("/api/runs/{}/research", run.id),
		Value::Null,
	)
	.await;
	let (pending_status, pending) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		"/api/capabilities/approvals?state=pending&role=approver",
		Value::Null,
	)
	.await;
	let (grants_status, grants) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		"/api/capabilities/approvals?state=active&role=approver",
		Value::Null,
	)
	.await;
	let (_, bob) = request(
		&r.c.app,
		&r.c.f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	let bob = bob["token"].as_str().unwrap();
	let (bob_run, _) = request(
		&r.c.app,
		bob,
		"GET",
		&format!("/api/runs/{}", run.id),
		Value::Null,
	)
	.await;
	let (bob_research, _) = request(
		&r.c.app,
		bob,
		"GET",
		&format!("/api/runs/{}/research", run.id),
		Value::Null,
	)
	.await;

	// Assert
	assert_eq!(status, 200, "{research}");
	assert!(
		research["usage"]["search_attempts"].as_u64().unwrap() >= 1,
		"{research}"
	);
	assert_eq!(research["limits"]["search_attempts"], 20);
	assert_eq!(research["limits"]["fetch_attempts"], 50);
	assert_eq!(
		research["estimated_micro_usd"], 0,
		"the fixture provider charges nothing"
	);
	assert_eq!(research["uncertain_attempts"], 0);
	assert_eq!(pending_status, 200, "{pending}");
	assert!(
		pending["items"]
			.as_array()
			.unwrap()
			.iter()
			.all(|item| item["state"] == "pending" && item["id"] != json!(first)),
		"a decided approval leaves the inbox: {pending}"
	);
	assert_eq!(grants_status, 200, "{grants}");
	let grant = grants["items"]
		.as_array()
		.unwrap()
		.iter()
		.find(|item| item["id"] == decided["grant_id"])
		.unwrap_or_else(|| panic!("{grants}"));
	assert_eq!(grant["kind"], "grant");
	assert_eq!(grant["action"], "research.disclose");
	assert_eq!(grant["operation"], "search");
	assert_eq!(grant["provider"], "research.fixture@1");
	assert_eq!(
		bob_research, bob_run,
		"the research view follows exactly the Run's read authority"
	);
	r.close().await;
}
