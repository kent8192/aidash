//! Web disclosure approval through the real Harness, worker authority and
//! approval HTTP API: a scripted model calls the research fixture package,
//! whose disclosure waits for an exact approval that one dispatch consumes.
use super::*;
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_domain::run_state::ApprovalRef;
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
	/// The Tenant's installed copy of the fixture provider's search tool.
	search_tool: Value,
}

#[rstest::fixture]
fn research_requests() -> Arc<AtomicUsize> {
	Arc::new(AtomicUsize::new(0))
}
/// The first page a URL intent asks the Run to read, found in the step state
/// of a model request, including JSON-encoded context.
fn intent_page(value: &Value) -> Option<String> {
	use aidash_domain::research::intent::URL_INTENT_INSTRUCTION;
	match value {
		Value::String(text) if text.contains(URL_INTENT_INSTRUCTION) => {
			intent_page(&serde_json::from_str::<Value>(text).ok()?)
		}
		Value::Object(fields) => match fields.get("url_intents") {
			Some(intents) if intents["instruction"] == URL_INTENT_INSTRUCTION => {
				intents["pages"][0].as_str().map(str::to_owned)
			}
			_ => fields.values().find_map(intent_page),
		},
		Value::Array(items) => items.iter().find_map(intent_page),
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
	c.policy["subjects"][aidash_server::domain::qualified_agent(
		&aidash_domain::nodes::NodeAddress::derived(c.f.config.deployment.as_str(), "acme"),
		"research",
		VERSION,
	)] = json!({"kind":"agent", "node": aidash_domain::nodes::NodeName::derived("acme")});
	crate::common::replace_policy(&c.f.store, "acme", c.policy.clone()).await;
	let bundle =
		c.f.registry
			.get(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				"aidash.bundle.research_fixture",
				"1.1.0",
			)
			.await
			.unwrap();
	let members: Vec<Value> = bundle.config["members"].as_array().unwrap().clone();
	// search, fetch, find and view.
	assert_eq!(members.len(), 4, "{members:?}");
	let approvals: Vec<Value> = std::iter::once(json!({"id":bundle.id,"version":bundle.version}))
		.chain(
			members
				.iter()
				.map(|member| json!({"id":member["id"],"version":member["version"]})),
		)
		.map(|entry| json!({"entry":entry,"enabled":true,"expected_revision":0}))
		.collect();
	let (status, approved) = request(
		&c.app,
		&c.token,
		"POST",
		"/api/authorization/acme/catalog/approvals",
		json!({"node":aidash_domain::nodes::NodeName::derived("acme"), "approvals": approvals }),
	)
	.await;
	assert_eq!(status, 200, "{approved}");
	let mut agent =
		c.f.registry
			.get(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				"research",
				"1.1.0",
			)
			.await
			.unwrap();
	agent.version = VERSION.into();
	agent.binding_normalization = None;
	agent.config["instructions"] = json!("Research with the bound tools.");
	agent.config["bindings"] = json!([{"kind":"bundle","target":{"registry_deployment":c.f.config.deployment,"id":bundle.id,"version":bundle.version},"narrow":{}}]);
	agent.config["remove_default"] = json!([]);
	for (method, path, body) in [
		("POST", "/api/registry".to_owned(), json!(agent)),
		(
			"POST",
			"/api/authorization/acme/catalog".to_owned(),
			json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"research","version":VERSION},"expected_revision":0,"enabled":true}),
		),
	] {
		let (status, value) = request(&c.app, &c.token, method, &path, body).await;
		assert_eq!(status, 200, "{value}");
	}
	let search_tool = members
		.iter()
		.find(|member| member["id"].as_str().unwrap().ends_with(".search"))
		.map(|member| json!({"id":member["id"],"version":member["version"]}))
		.unwrap();
	Research {
		c,
		_provider: provider.await,
		requests,
		search_tool,
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
			"registry",
			"registry_entry",
			"message",
			"agent",
			"model",
			"skill",
			"memory",
			"working_area",
			"artifact",
			"human_request",
			"bundle",
			"source"
		]);
		// Removing research disclosure authority keeps default metadata readable.
		policy["policies"].as_array_mut().unwrap().push(json!({"id":"default-decider-access","effect":"allow","subjects":{"any":true},"actions":["registry.read","decision.invoke"],"resources":{"kinds":["decider"]}}));
		policy["policies"].as_array_mut().unwrap().push(json!({"id":"approvable-research","effect":"allow","subjects":{"any":true},"actions":["capability.request","capability.approve"],"resources":{"kinds":["research"]}}));
		crate::common::replace_policy(&self.c.f.store, "acme", self.c.policy.clone()).await;
	}
	async fn register_search_only_agent(&mut self) {
		let node = self.c.f.config.deployment.to_string();
		self.c.policy["subjects"][aidash_server::domain::qualified_agent(
			&aidash_domain::nodes::NodeAddress::derived(&node, "acme"),
			"research",
			"3.1.0",
		)] = json!({"kind":"agent", "node": aidash_domain::nodes::NodeName::derived("acme")});
		let mut agent = self
			.c
			.f
			.registry
			.get(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				"research",
				VERSION,
			)
			.await
			.unwrap();
		agent.version = "3.1.0".into();
		agent.binding_normalization = None;
		agent.config["bindings"] = json!([{"kind":"tool","target":{"registry_deployment":node,"id":self.search_tool["id"],"version":self.search_tool["version"]},"narrow":{}}]);
		crate::common::replace_policy(&self.c.f.store, "acme", self.c.policy.clone()).await;
		for (path, body) in [
			("/api/registry", json!(agent)),
			(
				"/api/authorization/acme/catalog",
				json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"research","version":"3.1.0"},"expected_revision":0,"enabled":true}),
			),
		] {
			let (status, value) = request(&self.c.app, &self.c.token, "POST", path, body).await;
			assert_eq!(status, 200, "{path}: {value}");
		}
	}
	async fn delegate(&self) -> Run {
		let (status, value) = request(
			&self.c.app,
			&self.c.token,
			"POST",
			&format!("/api/tasks/{}/delegate", self.c.task),
			json!({"node_id":self.c.f.config.deployment,"agent":{"id":"research","version":VERSION}}),
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
				&& let WaitingState::Approval {
					approval: ApprovalRef::Capability { approval_id },
					..
				} = waiting.as_ref()
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
					.fetch_all(self.c.f.store.pool.fixture_driver())
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
			.fetch_one(self.c.f.store.pool.fixture_driver())
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

	// The approved call runs as asked, so the model is never asked to repeat
	// it: exactly one dispatch consumes the approval, and the following
	// identical call waits for a new one.
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
	assert_eq!(
		statuses,
		["ok"],
		"the wait for approval is no result the model reads"
	);
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
	assert_eq!(
		r.requests.load(Ordering::SeqCst),
		2,
		"one model response asked for each search"
	);
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
	let workspace =
		r.c.f
			.store
			.task(&TEST_FIXTURE, r.c.task)
			.await
			.unwrap()
			.workspace_id;
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
	// An Agent version that binds Web search but no Page reading cannot act
	// on one, and nothing is created.
	r.register_search_only_agent().await;
	let (status, refused) = request(
		&r.c.app,
		&r.c.token,
		"POST",
		&path,
		json!({"idempotency_key":Uuid::new_v4(),"agent_version":"3.1.0","title":"Notes","description":"Summarize them.","read_pages":[page]}),
	)
	.await;
	assert_eq!(status, 400, "{refused}");
	assert!(
		refused
			.to_string()
			.contains("binds no page reading operation"),
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
			.task(
				&TEST_FIXTURE,
				r.c.f.store.run(&TEST_FIXTURE, run).await.unwrap().task_id,
			)
			.await
			.unwrap();
	assert_eq!(
		task.description, "Summarize them.",
		"the person's text is stored as written"
	);
	let (status, research) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		&format!("/api/runs/{run}/research"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{research}");
	assert_eq!(research["url_intents"][0]["input"], "task");
	assert_eq!(research["url_intents"][0]["pages"], json!([page]));
	// Steering records its own URL intent beside the message text.
	let steered_page = "https://example.org/rollback";
	let (status, steered) = request(
		&r.c.app,
		&r.c.token,
		"POST",
		&format!("/api/working-areas/{}/steer", area["id"].as_str().unwrap()),
		json!({"idempotency_key":Uuid::new_v4(),"expected_run_id":run,"content":"Also cover the rollback.","read_pages":[steered_page]}),
	)
	.await;
	assert_eq!(status, 200, "{steered}");
	let (_, research) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		&format!("/api/runs/{run}/research"),
		Value::Null,
	)
	.await;
	assert_eq!(research["url_intents"][1]["input"], "run_message");
	assert_eq!(research["url_intents"][1]["pages"], json!([steered_page]));
	let inputs = r.c.f.store.run_inputs(&TEST_FIXTURE, run).await.unwrap();
	assert_eq!(
		inputs.last().unwrap().content,
		"Also cover the rollback.",
		"the steering text is stored as written"
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
	assert!(card.get("account_id").is_none(), "{card}");
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
	let (status, too_short) = request(
		&r.c.app,
		&r.c.token,
		"POST",
		&format!("/api/capabilities/approvals/{first}/decide"),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":card["revision"],"choice":"allow_run","targets":card["targets"],"expires_at":chrono::Utc::now() + chrono::Duration::seconds(30)}),
	)
	.await;
	assert_eq!(
		status, 400,
		"a Run grant lasts at least a minute: {too_short}"
	);
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
	let (_, bob) = async {
		(
			200_u16,
			common::issued_credential(&r.c.f.store, "acme", "bob").await,
		)
	}
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

	// Once its `expires_at` passes, the grant reads as expired in the
	// Thread's unfiltered view and leaves the active one.
	{
		use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
		let grant_id: Uuid = serde_json::from_value(decided["grant_id"].clone()).unwrap();
		let sql = Query::update()
			.table(Alias::new("core_records"))
			.value(
				Alias::new("expires_at"),
				chrono::Utc::now() - chrono::Duration::seconds(1),
			)
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(grant_id)))
			.to_string(PostgresQueryBuilder);
		sqlx::query(&sql)
			.execute(r.c.f.store.pool.fixture_driver())
			.await
			.unwrap();
	}
	let (all_status, all) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		"/api/capabilities/approvals",
		Value::Null,
	)
	.await;
	let (_, active) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		"/api/capabilities/approvals?state=active&role=approver",
		Value::Null,
	)
	.await;
	assert_eq!(all_status, 200, "{all}");
	let elapsed = all["items"]
		.as_array()
		.unwrap()
		.iter()
		.find(|item| item["id"] == decided["grant_id"])
		.unwrap_or_else(|| panic!("{all}"));
	assert_eq!(elapsed["state"], "expired", "{elapsed}");
	assert!(
		active["items"]
			.as_array()
			.unwrap()
			.iter()
			.all(|item| item["id"] != decided["grant_id"]),
		"{active}"
	);
	r.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_disclosure_whose_run_ended_is_closed_in_every_view(
	#[future] research_fixture: Research,
) {
	// Arrange: the Run waits on a disclosure approval and is then cancelled.
	let mut r = research_fixture.await;
	let run = r.delegate().await;
	r.require_disclosure_approval().await;
	let approval = r.until_waiting(run.id).await;
	{
		use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
		let sql = Query::update()
			.table(Alias::new("runs"))
			.value(Alias::new("control"), "CANCELLED")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(run.id)))
			.to_string(PostgresQueryBuilder);
		sqlx::query(&sql)
			.execute(r.c.f.store.pool.fixture_driver())
			.await
			.unwrap();
	}

	// Act: the Thread reads the unfiltered list; the approver its inbox.
	let (status, all) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		"/api/capabilities/approvals",
		Value::Null,
	)
	.await;
	let (inbox_status, inbox) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		"/api/capabilities/approvals?state=pending&role=approver",
		Value::Null,
	)
	.await;

	// Assert: no view offers it as pending.
	assert_eq!(status, 200, "{all}");
	let card = all["items"]
		.as_array()
		.unwrap()
		.iter()
		.find(|item| item["id"] == json!(approval))
		.unwrap_or_else(|| panic!("{all}"));
	assert_eq!(card["state"], "closed", "{card}");
	assert_eq!(card["closed_reason"], "run_ended", "{card}");
	assert_eq!(inbox_status, 200, "{inbox}");
	assert!(
		inbox["items"]
			.as_array()
			.unwrap()
			.iter()
			.all(|item| item["id"] != json!(approval)),
		"{inbox}"
	);
	r.close().await;
}

const PAGE: &str = "https://research-fixture.example/strasse";

/// The first model request reads the fixture page's first line; once a
/// delivered fragment is in the conversation, the answer cites it.
fn citing_router() -> Arc<Router> {
	citing_router_for(PAGE)
}

/// [`citing_router`] for another fixture page.
fn citing_router_for(page: &'static str) -> Arc<Router> {
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
								"name":"research_fetch","arguments":json!({"url":page,"max_bytes":5}).to_string()
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
			.fetch_all(self.c.f.store.pool.fixture_driver())
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
		status, 403,
		"a Deployment Administrator reads no Tenant's evidence (#484)"
	);
	assert!(
		!deployment_administrator.to_string().contains("Alpha"),
		"{deployment_administrator}"
	);

	// Everyone else gets one identical absence, whatever the reason. Bob is
	// a member of the Tenant whose policy no longer lets him read Runs.
	r.c.policy["policies"].as_array_mut().unwrap().push(json!({"id":"bob-reads-no-runs","effect":"deny","subjects":{"ids":["bob"]},"actions":["run.read"],"resources":{"kinds":["run"]}}));
	crate::common::replace_policy(&r.c.f.store, "acme", r.c.policy.clone()).await;
	let (_, bob) = async {
		(
			200_u16,
			common::issued_credential(&r.c.f.store, "acme", "bob").await,
		)
	}
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

#[rstest::rstest]
#[tokio::test]
async fn the_providers_api_says_what_is_set_up_and_approved_only_to_a_tenant_subject(
	#[future] research_fixture: Research,
) {
	// Arrange: the fixture provider is set up and its package approved for
	// this Tenant; Brave and public page reading are not set up.
	let r = research_fixture.await;

	// Act
	let (status, providers) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		"/api/research/providers",
		Value::Null,
	)
	.await;
	let (deployment_administrator_status, _) = request(
		&r.c.app,
		&r.c.f.config.api_token,
		"GET",
		"/api/research/providers",
		Value::Null,
	)
	.await;

	// Assert
	assert_eq!(status, 200, "{providers}");
	let provider = |name: &str| {
		providers["providers"]
			.as_array()
			.unwrap()
			.iter()
			.find(|item| item["provider"] == name)
			.unwrap_or_else(|| panic!("{providers}"))
			.clone()
	};
	let fixture = provider("research.fixture@1");
	assert_eq!(fixture["set_up"], true);
	let approved = |operation: &str| {
		fixture["operations"]
			.as_array()
			.unwrap()
			.iter()
			.find(|item| item["operation"] == operation)
			.unwrap()["approved_definitions"]
			.clone()
	};
	// The Tenant's installed copy, whatever its identifier, is approved.
	assert_eq!(approved("search"), json!([r.search_tool]));
	assert_eq!(approved("fetch").as_array().unwrap().len(), 1);
	assert_eq!(approved("find").as_array().unwrap().len(), 1);
	assert_eq!(provider("research.brave@1")["set_up"], false);
	assert_eq!(provider("research.page@1")["set_up"], false);
	assert_eq!(
		provider("research.page@1")["operations"][0]["approved_definitions"],
		json!([]),
		"no package of the page provider is approved for this Tenant"
	);
	assert!(
		!providers.to_string().contains("AIDASH_"),
		"never names configuration: {providers}"
	);
	assert_eq!(providers["limits"]["search_attempts"], 20);
	assert_eq!(
		deployment_administrator_status, 403,
		"Deployment Administrators read no Tenant's research setup"
	);
	// A second Node starts without approvals, regardless of the default Catalog.
	let (status, second) = common::BrowserMapping::new(&r.c.f, "acme", "alice")
		.await
		.send(
			"POST",
			"/api/tenants/acme/nodes",
			json!({"label":"Research Node"}),
		)
		.await;
	assert_eq!(status, 201, "{second}");
	let name = second["name"].as_str().unwrap();
	let path = format!("/api/research/providers?node={name}");
	let (status, empty) = request(&r.c.app, &r.c.token, "GET", &path, Value::Null).await;
	assert_eq!(status, 200, "{empty}");
	assert!(
		empty["providers"]
			.as_array()
			.unwrap()
			.iter()
			.all(|provider| provider["operations"]
				.as_array()
				.unwrap()
				.iter()
				.all(|operation| operation["approved_definitions"] == json!([]))),
		"another Node's approvals must not appear: {empty}"
	);
	let (status, approved) = request(
		&r.c.app,
		&r.c.token,
		"POST",
		"/api/authorization/acme/catalog/approvals",
		json!({"node":name,"approvals":[{"entry":r.search_tool,"enabled":true,"expected_revision":0}]}),
	)
	.await;
	assert_eq!(status, 200, "{approved}");
	let (status, selected) = request(&r.c.app, &r.c.token, "GET", &path, Value::Null).await;
	assert_eq!(status, 200, "{selected}");
	let selected = selected["providers"]
		.as_array()
		.unwrap()
		.iter()
		.find(|provider| provider["provider"] == "research.fixture@1")
		.unwrap();
	let selected = selected["operations"]
		.as_array()
		.unwrap()
		.iter()
		.find(|operation| operation["operation"] == "search")
		.unwrap();
	assert_eq!(selected["approved_definitions"], json!([r.search_tool]));
	let mut denied = r.c.policy.clone();
	denied["policies"].as_array_mut().unwrap().push(json!({"id":"no-research-node","effect":"deny","subjects":{"any":true},"actions":["node.use"],"resources":{"kinds":["node"],"ids":[name]}}));
	common::replace_policy(&r.c.f.store, "acme", denied).await;
	let (status, body) = request(&r.c.app, &r.c.token, "GET", &path, Value::Null).await;
	assert_eq!(status, 403, "{body}");
	let (status, body) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		"/api/research/providers?node=able-oak-00000000",
		Value::Null,
	)
	.await;
	// Absent and inaccessible Nodes share the same refusal to avoid disclosing existence.
	assert_eq!(status, 403, "{body}");
	assert_eq!(body["error"], "forbidden");

	r.close().await;
}

#[path = "web_research/retention.rs"]
mod retention;
#[path = "web_research/view.rs"]
mod view;
