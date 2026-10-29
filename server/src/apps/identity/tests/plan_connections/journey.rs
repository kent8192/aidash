//! HTTP admission, durable owner authority and streamed Native plan inference.
use super::*;
use aidash_application::{
	ports::{InferenceProgressSink, ModelProvider},
	provider_access::Access,
	tenancy::TEST_FIXTURE,
};
use aidash_domain::{
	context::ContextEvent,
	projection::native::native_context,
	provider::{ModelContext, ModelRequest, ToolSpec, progress::InferenceProgress},
};
use aidash_integrations::{inference::responses::ResponsesProvider, keyless::Clients};
use aidash_server::apps::identity::models::AuthorizationExecution;
use reinhardt::db::orm::Model;
use std::time::Duration;

#[rstest::fixture]
fn salted_plan_runtime(
	#[default(Parts::default())] _parts: Parts,
	#[from(plan_runtime)]
	#[with(_parts.clone())]
	runtime: common::RuntimeFuture,
) -> common::RuntimeFuture {
	async move {
		let mut runtime = runtime.await;
		runtime.federation.store.cache_salt = Some(
			aidash_integrations::inference::CacheSaltKeys::new(
				&[aidash_integrations::inference::CacheSaltKey {
					version: 1,
					secret: "journey-cache-salt-fixture".into(),
				}],
				1,
			)
			.unwrap(),
		);
		runtime
	}
	.boxed()
	.shared()
}

#[rstest::fixture]
fn journey_application(
	#[default(Parts::default())] _parts: Parts,
	#[from(salted_plan_runtime)]
	#[with(_parts.clone())]
	_runtime: common::RuntimeFuture,
	#[from(plan_application)]
	#[with(_parts.clone(), _runtime.clone())]
	application: common::ApplicationFuture,
) -> common::ApplicationFuture {
	application
}

/// Exercise real admission/token resolution and substitute only the transport
/// destination after it succeeds. No grant, pin or bearer is manufactured here.
struct LocalResponses {
	admitted: AdmittedAccess,
	endpoint: String,
}
#[async_trait]
impl ProviderAccess for LocalResponses {
	async fn resolve(&self, context: &Context, endpoint: &str, source: &Source) -> Result<Access> {
		let mut access = self.admitted.resolve(context, endpoint, source).await?;
		assert_eq!(access.endpoint, Provider::Chatgpt.base_url());
		access.endpoint.clone_from(&self.endpoint);
		Ok(access)
	}
	async fn check(&self, context: &Context, endpoint: &str, source: &Source) -> Result<()> {
		self.admitted.check(context, endpoint, source).await
	}
	async fn plan_rejected(
		&self,
		context: &Context,
		disposition: aidash_domain::plan_connections::Disposition,
	) -> Result<bool> {
		self.admitted.plan_rejected(context, disposition).await
	}
}

#[derive(Default)]
struct Progress(Mutex<Vec<InferenceProgress>>);
impl InferenceProgressSink for Progress {
	fn offer(&self, progress: InferenceProgress) {
		self.0.lock().unwrap().push(progress);
	}
}

async fn owner_request(fixture: &Fixture, token: &str, path: &str, body: Value) -> Value {
	let (status, body) = common::request(&fixture.app, token, "POST", path, body).await;
	assert_eq!(status, 200, "{path}: {body}");
	body
}

fn projected_request(run: &aidash_domain::Run) -> ModelRequest {
	ModelRequest {
		instructions: "Inspect both files, then report their results.".into(),
		context: ModelContext::Native(native_context(
			&run.context,
			&json!({"task":{"id":run.task_id,"description":"Read two files"},"step":run.step}),
		)),
		tools: vec![ToolSpec {
			name: "file_read".into(),
			description: "Read a file".into(),
			parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
		}],
		max_output_tokens: 1024,
		cache_scope: Some(aidash_domain::projection::CacheScope {
			tenant: "alpha".into(),
			key_version: 1,
		}),
		..Default::default()
	}
}

/// A repository-only admission test cannot catch a router dropping the root
/// credential, a claim failing to commit its pin, or a resumed adapter omitting
/// the persisted Native tool turn. This journey crosses those real boundaries.
#[rstest::rstest]
#[tokio::test]
async fn http_claim_persists_owner_authority_for_native_streams_and_disconnect_stops_io(
	#[from(parts)] parts: Parts,
	#[future(awt)]
	#[from(journey_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	// Arrange: consent through the real router and fake OpenAI's PKCE exchange.
	let fixture = Fixture::new(application, parts);
	let (status, dashboard_context) = map(&fixture, "browser-a", "alice").await;
	assert_eq!(status, 200);
	let connected = connect(&fixture, "browser-a", &dashboard_context).await;
	assert_eq!(fixture.openai.calls.lock().unwrap().exchanges, 1);
	let model: aidash_server::registry::Entry = serde_json::from_value(json!({
		"id":"journey-model","version":"1.0.0","kind":"model",
		"name":{"en":"Native plan model"},"description":{"en":"Admission journey"},
		"config":{"provider":"chatgpt","model_id":"gpt-5.5","endpoint":"https://api.openai.com/v1",
			"plan_connection":"chatgpt","context_window":128000,"max_output_tokens":1024,
			"projection_versions":["native"],"modalities":["text"],"cost":{}}
	}))
	.unwrap();
	let agent: aidash_server::registry::Entry = serde_json::from_value(json!({
		"id":"journey-agent","version":"1.0.0","kind":"agent",
		"name":{"en":"Native plan agent"},"description":{"en":"Admission journey"},
		"config":{"schema_version":1,"model":{"id":"journey-model","version":"1.0.0"},
			"projection_version":"native","instructions":"Read both files","bindings":[],"remove_default":[]}
	}))
	.unwrap();
	let tenant = aidash_domain::tenancy::TenantName::new("alpha").unwrap();
	for entry in [model.clone(), agent.clone()] {
		fixture.f.registry.register(&tenant, entry).await.unwrap();
	}
	let authorization = Policies {
		pool: fixture.f.store.pool.clone(),
	};
	let snapshot = authorization.snapshot("alpha").await.unwrap();
	let mut policy = serde_json::to_value(&snapshot.bundle).unwrap();
	policy["policies"] = json!([{"id":"journey-work","effect":"allow","subjects":{"any":true},
		"actions":["*"],"resources":{"kinds":["*"]}}]);
	owner_request(
		&fixture,
		&fixture.f.config.api_token,
		"/api/authorization/alpha",
		json!({"expected_revision":snapshot.revision,"bundle":policy}),
	)
	.await;
	common::approve_agent(&fixture.f, "alpha", &agent).await;
	let credential = common::issued_credential(&fixture.f.store, "alpha", "alice").await;
	let token = credential["token"].as_str().unwrap();
	let workspace = owner_request(
		&fixture,
		token,
		"/api/workspaces",
		json!({"title":"Plan journey","goal":"Read two files"}),
	)
	.await;
	let task = owner_request(
		&fixture,
		token,
		&format!(
			"/api/workspaces/{}/tasks",
			workspace["id"].as_str().unwrap()
		),
		json!({"title":"Two files","description":"Inspect both files"}),
	)
	.await;

	// Act: the HTTP claim, rather than a fabricated grant or direct admit call,
	// must atomically create the executable Run, ExecutionGrant and Plan pin.
	owner_request(
		&fixture,
		token,
		&format!("/api/tasks/{}/claim", task["id"].as_str().unwrap()),
		json!({"revision":task["revision"],"agent":{"id":"journey-agent","version":"1.0.0"}}),
	)
	.await;
	let run = fixture.f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	let grant = fixture
		.f
		.store
		.control_pool
		.atomic(&TenancyScope::tenant("alpha").unwrap(), async |tx| {
			Ok(AuthorizationExecution::objects()
				.filter(AuthorizationExecution::field_run_id().eq(run.id))
				.get_with_db(tx)
				.await?)
		})
		.await
		.unwrap();
	assert_eq!(grant.root_subject, "alice");
	assert_eq!(
		grant.subject_chain.first().map(String::as_str),
		Some("alice")
	);
	assert_eq!(
		grant.credential_id().to_string(),
		credential["credential"]["id"].as_str().unwrap()
	);
	assert_eq!(grant.task_id(), run.task_id);
	assert_eq!(grant.workspace_id(), run.workspace_id);
	let pin = pinned(
		&fixture.f.store.control_pool,
		"alpha",
		run.id,
		Provider::Chatgpt,
	)
	.await
	.unwrap();
	assert_eq!(
		pin.identity,
		identity_of(&fixture.f, "plan-connection-alice").await
	);
	assert_eq!(
		pin.connection.to_string(),
		connected["connection"]["id"].as_str().unwrap()
	);

	let seen = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
	let captured = seen.clone();
	let router = reinhardt::test::stub::StubRouter::new().route("/responses", http::Method::POST,
		common::upstream_fixtures::reply(move |request: reinhardt::Request| {
			let captured = captured.clone();
			let body = request.json::<Value>().unwrap();
			let bearer = request.headers[http::header::AUTHORIZATION].to_str().unwrap().to_owned();
			async move {
				let mut seen = captured.lock().unwrap();
				seen.push((bearer, body));
				let output = if seen.len() == 1 { json!([
					{"type":"reasoning","encrypted_content":"journey-opaque-reasoning"},
					{"type":"function_call","namespace":"aidash","call_id":"journey-call-a","name":"file_read","arguments":"{\"path\":\"a.txt\"}"},
					{"type":"function_call","namespace":"aidash","call_id":"journey-call-b","name":"file_read","arguments":"{\"path\":\"b.txt\"}"}
				]) } else { json!([{"type":"message","content":[{"type":"output_text","text":"Both files inspected."}]}]) };
				let delta = if seen.len() == 1 {
					json!({"type":"response.reasoning_summary_text.delta","delta":"journey-opaque-reasoning"})
				} else {
					json!({"type":"response.output_text.delta","delta":"Both files inspected."})
				};
				let completed = json!({"type":"response.completed","response":{"status":"completed","output":output,
					"usage":{"input_tokens":100,"output_tokens":20,"output_tokens_details":{"reasoning_tokens":3}}}});
				reinhardt::Response::ok().with_header("content-type", "text/event-stream")
					.with_body(format!("data: {delta}\n\ndata: {completed}\n\n"))
			}
		})).into_server_router();
	let upstream = common::upstream_fixtures::upstream(Arc::new(router)).await;
	let provider = ResponsesProvider {
		clients: Clients::new(
			reqwest::Client::new(),
			Duration::from_secs(30),
			Duration::from_secs(5),
		)
		.unwrap(),
		config: serde_json::from_value(model.config).unwrap(),
		access: Arc::new(LocalResponses {
			admitted: AdmittedAccess {
				store: fixture.f.store.clone(),
			},
			endpoint: upstream.url.clone(),
		}),
		context: Context {
			tenant: "alpha".into(),
			run: Some(run.id),
			..Default::default()
		},
		cache_salt: fixture.f.store.cache_salt.clone(),
	};
	let progress = Progress::default();
	let response = provider
		.infer(projected_request(&run), &progress)
		.await
		.unwrap();
	assert_eq!(
		response
			.tool_calls
			.iter()
			.map(|call| call.id.as_str())
			.collect::<Vec<_>>(),
		["journey-call-a", "journey-call-b"]
	);

	// Persist the actual provider calls and deterministic mock tool results
	// through the production lease-fenced projection + Context Journal save.
	let worker = Uuid::new_v4();
	let mut leased = fixture
		.f
		.store
		.lease_run(worker, 60)
		.await
		.unwrap()
		.unwrap();
	assert_eq!(leased.id, run.id);
	leased.context.push(ContextEvent::Assistant {
		provider_activity: vec![],
		epoch: i64::from(leased.step),
		text: response.text,
		calls: response
			.tool_calls
			.iter()
			.map(|call| call.id.clone())
			.collect(),
	});
	for call in response.tool_calls {
		let result = json!({"path":call.arguments["path"],"text":"fixture file contents"});
		leased.context.push(ContextEvent::tool(call, result));
	}
	fixture
		.f
		.store
		.save_run(&TEST_FIXTURE, &leased, worker, "run.sources_observed")
		.await
		.unwrap();
	drop(leased);
	let resumed = fixture.f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	assert_eq!(resumed.context.history.len(), 3);
	let journal = table_text(&fixture.f, "run_context_events").await;
	assert!(journal.contains("journey-call-a") && journal.contains("journey-call-b"));
	assert!(!journal.contains("journey-opaque-reasoning"));
	let next_request = projected_request(&resumed);
	let response = provider
		.infer(next_request.clone(), &progress)
		.await
		.unwrap();
	assert_eq!(response.text, "Both files inspected.");
	{
		let seen = seen.lock().unwrap();
		assert_eq!(seen.len(), 2);
		for (bearer, body) in seen.iter() {
			assert_eq!(bearer, "Bearer plan-access-1");
			assert_eq!(body["model"], "gpt-5.5");
			assert_eq!(body["store"], false);
			assert_eq!(body["stream"], true);
			assert!(body.get("previous_response_id").is_none());
			assert!(!body.to_string().contains("journey-opaque-reasoning"));
		}
		let items = seen[1].1["input"].as_array().unwrap();
		let calls = items
			.iter()
			.filter(|item| {
				item["type"] == "function_call" || item["type"] == "function_call_output"
			})
			.map(|item| {
				(
					item["type"].as_str().unwrap(),
					item["call_id"].as_str().unwrap(),
				)
			})
			.collect::<Vec<_>>();
		assert_eq!(
			calls,
			[
				("function_call", "journey-call-a"),
				("function_call", "journey-call-b"),
				("function_call_output", "journey-call-a"),
				("function_call_output", "journey-call-b")
			]
		);
		for (item, path) in items
			.iter()
			.filter(|item| item["type"] == "function_call_output")
			.zip(["a.txt", "b.txt"])
		{
			let output: Value = serde_json::from_str(item["output"].as_str().unwrap()).unwrap();
			assert_eq!(output["content"]["path"], path);
			assert_eq!(output["content"]["text"], "fixture file contents");
		}
	}
	assert!(progress.0.lock().unwrap().iter().any(
		|item| matches!(item, InferenceProgress::Text { text } if text == "Both files inspected.")
	));
	assert!(!format!("{:?}", progress.0.lock().unwrap()).contains("journey-opaque-reasoning"));

	// The same admitted Run must stop before transport after HTTP disconnect.
	let status = send(
		&fixture,
		"browser-a",
		&dashboard_context,
		"GET",
		PLAN,
		Value::Null,
	)
	.await;
	let disconnected = send(
		&fixture,
		"browser-a",
		&dashboard_context,
		"DELETE",
		PLAN,
		json!({"expected_revision":json_body(&status)["connection"]["revision"]}),
	)
	.await;
	assert_eq!(disconnected.status(), 200, "{}", json_body(&disconnected));
	assert!(matches!(
		provider.infer(next_request, &progress).await,
		Err(Error::Plan(Failure::NotConnected))
	));
	assert_eq!(
		seen.lock().unwrap().len(),
		2,
		"disconnect must stop before any provider HTTP"
	);
	fixture.cleanup().await;
}
