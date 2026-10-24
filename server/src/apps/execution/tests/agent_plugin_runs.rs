//! A Run of an Agent that takes in an Agent Plugin lists the plugin's remote MCP server at its
//! first step, pins the tools it offers, asks for a Tool call approval before it calls a tool the
//! Agent did not declare Read-only, and ends its sessions when it finishes (ADR 0072, ADR 0073).
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
use aidash_application::tenancy::{TEST_FIXTURE, TenancyScope};
use aidash_domain::agent_plugin::{
	self, AgentPluginRef, MCP_SCHEMA, PLUGIN_SCHEMA, PluginFile, PluginName, ServerName,
	model_alias, schema_digest,
};
use aidash_server::{
	authorization::Authorization,
	domain::{Run, RunControlAction},
	federation::Federation,
	harness::Harness,
	registry::EntityRef,
};
use common::*;
use http::Method;
use reinhardt::test::stub::StubRouter;
use serde_json::{Value, json};
use std::sync::{
	Arc, Mutex,
	atomic::{AtomicBool, Ordering::SeqCst},
};
#[path = "support/legacy.rs"]
mod common;
#[path = "support/mcp.rs"]
mod mcp;

const PLUGIN: &str = "acme-tools";
const SERVER: &str = "docs";
const SOURCE: &str = "https://github.com/acme/tools/tree/0123456789abcdef0123456789abcdef01234567";

fn alias(tool: &str) -> String {
	model_alias(
		&PluginName::try_from(PLUGIN.to_owned()).unwrap(),
		&ServerName::try_from(SERVER.to_owned()).unwrap(),
		tool,
	)
}

/// The plugin copy whose `mcp.json` names one remote server at `url`.
fn plugin_files(url: &str) -> Vec<PluginFile> {
	let file = |path: &str, value: Value| PluginFile {
		path: path.to_owned(),
		bytes: value.to_string().into_bytes(),
	};
	vec![
		file(
			"plugin.json",
			json!({"$schema": PLUGIN_SCHEMA, "name": PLUGIN, "version": "1.0.0"}),
		),
		file(
			"mcp.json",
			json!({"$schema": MCP_SCHEMA, "mcpServers": {SERVER: {"type": "streamable-http", "url": url}}}),
		),
	]
}

/// Keeps the model's view of each request: the tools it was offered.
#[derive(Default)]
struct Model {
	only_update: AtomicBool,
	sampling: Mutex<Vec<Value>>,
	offered: Mutex<Vec<Vec<String>>>,
}

impl Model {
	/// Calls every plugin tool the first time it can, the Read-only one first, then finishes.
	fn reply(&self, request: &reinhardt::Request) -> reinhardt::Response {
		let body = request.json::<Value>().unwrap();
		if body["messages"][0]["content"] == "sampling-only-system" {
			assert!(body.get("tools").is_none());
			assert!(body.to_string().contains("Server sampling"));
			assert!(!body.to_string().contains("Look it up, then update it"));
			self.sampling.lock().unwrap().push(body);
			return reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"sample completion"}}],"usage":{"prompt_tokens":2,"completion_tokens":3}})).unwrap();
		}

		let offered: Vec<String> = body["tools"]
			.as_array()
			.into_iter()
			.flatten()
			.filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
			.collect();
		self.offered.lock().unwrap().push(offered.clone());
		let context: Value =
			serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
		let called = context["history"]
			.as_array()
			.unwrap()
			.iter()
			.filter(|event| event["kind"] == "tool")
			.count();
		let calls: Vec<Value> = ["lookup", "update"]
			.into_iter()
			.map(alias)
			.filter(|name| !self.only_update.load(SeqCst) || *name == alias("update"))
			.filter(|name| offered.contains(name))
			.enumerate()
			.map(|(n, name)| {
				json!({"id":format!("call-{n}"),"type":"function","function":{"name":name,"arguments":json!({"q":format!("call {n}")}).to_string()}})
			})
			.collect();
		let message = if called == 0 && !calls.is_empty() {
			json!({"role":"assistant","content":null,"tool_calls":calls})
		} else {
			json!({"role":"assistant","content":"Completed with the plugin tools"})
		};
		reinhardt::Response::ok()
			.with_json(&json!({"choices":[{"index":0,"finish_reason":if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"},"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
			.unwrap()
	}
}

struct World {
	f: Federation,
	token: String,
	app: TestApplication,
	run: Run,
	mcp: Arc<mcp::FakeMcp>,
	model: Arc<Model>,
	/// Keeps the loopback servers up until the test ends.
	_servers: Vec<Arc<reinhardt::test::fixtures::server::TestServerGuard>>,
	url: String,
	schema: String,
}

/// An Agent that takes in the plugin, whose `lookup` tool is declared Read-only, claimed by a
/// member under a policy that grants no direct permission to call a plugin tool. `down` points the
/// plugin's server at an address nothing listens on.
async fn world(fixture: ApplicationFixture, down: bool) -> World {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let mcp = mcp::FakeMcp::new(vec![mcp::tool("lookup"), mcp::tool("update")]);
	let model = Arc::new(Model::default());
	let router = {
		let model = model.clone();
		mcp.routes(StubRouter::new()).route(
			"/v1/chat/completions",
			Method::POST,
			upstream_fixtures::reply(move |request: reinhardt::Request| {
				let response = model.reply(&request);
				async move { response }
			}),
		)
	};
	let server = Arc::new(upstream_fixtures::upstream(Arc::new(router.into_server_router())).await);
	let endpoint = server.url.clone();
	f.store.keyless.permit_origin_of(&endpoint).unwrap();
	let mcp_url = if down {
		// An address that refused the connection: the listener is gone before the Run starts.
		let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = closed.local_addr().unwrap();
		drop(closed);
		let url = format!("http://{address}/mcp");
		f.store.keyless.permit_origin_of(&url).unwrap();
		url
	} else {
		format!("{endpoint}{}", mcp::PATH)
	};

	let mut policy = policy(f.config.deployment.as_str());
	require_tool_call_approval(&mut policy);
	tenant_root(&f, "acme").await;
	let deployment_administrator = &f.config.api_token;
	let (status, snapshot) = request(
		&app,
		deployment_administrator,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":0,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "initial policy: {snapshot}");

	// The copy is stored the way an import stores it; the Agent pins it by digest.
	let files = plugin_files(&mcp_url);
	let loaded = agent_plugin::load(&files).unwrap();
	let reference = AgentPluginRef::of(&loaded, SOURCE);
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let mut tx = f
		.store
		.pool
		.begin(&TenancyScope::Tenant(tenant.clone()))
		.await
		.unwrap();
	aidash_server::apps::registry::repositories::agent_plugins::insert(
		&mut *tx,
		SOURCE,
		&loaded,
		&files,
		&[],
	)
	.await
	.unwrap();
	tx.commit().await.unwrap();

	let lookup_digest = schema_digest(&mcp::schema());
	let authorization = Authorization {
		pool: f.store.pool.clone(),
	};
	for (kind, id, config) in [
		(
			"model",
			"model",
			json!({"provider":"openrouter","model_id":"fixture","endpoint":format!("{endpoint}/v1"),"context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}),
		),
		(
			"agent",
			"research",
			json!({
				"schema_version":1,"model":{"id":"model","version":"1.0.0"},
				"instructions":"Use the docs server","bindings":[],
				"remove_default":["file_search","file_read"],
				"agent_plugins":[reference],
				"agent_plugin_settings":{PLUGIN:{SERVER:{"read_only":[{"name":"lookup","schema_digest":lookup_digest}]}}}
			}),
		),
	] {
		let entry = json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"fixture"},"capabilities":[],"languages":["en"],"schema":{"type":"object"},"config":config});
		f.registry
			.register(&tenant, serde_json::from_value(entry).unwrap())
			.await
			.unwrap();
		authorization
			.set_catalog(
				"acme",
				&aidash_domain::nodes::NodeName::derived("acme"),
				&EntityRef {
					id: id.into(),
					version: "1.0.0".into(),
				},
				0,
				true,
				"deployment_administrator",
			)
			.await
			.unwrap();
	}

	let (status, credential) = request(
		&app,
		deployment_administrator,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	assert_eq!(status, 200);
	let token = credential["token"].as_str().unwrap().to_owned();
	let (status, workspace) = request(
		&app,
		&token,
		"POST",
		"/api/workspaces",
		json!({"title":"Plugin work","goal":"Use the docs server"}),
	)
	.await;
	assert_eq!(status, 200, "{workspace}");
	let (status, task) = request(
		&app,
		&token,
		"POST",
		&format!(
			"/api/workspaces/{}/tasks",
			workspace["id"].as_str().unwrap()
		),
		json!({"title":"Research","description":"Look it up, then update it"}),
	)
	.await;
	assert_eq!(status, 200, "{task}");
	let (status, claimed) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{}/claim", task["id"].as_str().unwrap()),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{claimed}");
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	World {
		f,
		token,
		app,
		run,
		mcp,
		model,
		_servers: vec![server],
		url,
		schema,
	}
}

async fn step_until(harness: &Harness, world: &World, mut done: impl FnMut(&Run) -> bool) -> Run {
	for _ in 0..60 {
		harness.worker_once().await.unwrap();
		let current = world
			.f
			.store
			.run(&TEST_FIXTURE, world.run.id)
			.await
			.unwrap();
		if done(&current) {
			return current;
		}
	}
	let current = world
		.f
		.store
		.run(&TEST_FIXTURE, world.run.id)
		.await
		.unwrap();
	panic!(
		"the Run did not reach the expected state: phase={} error={:?} state={:?}",
		current.phase().as_str(),
		current.error,
		current.state
	);
}

async fn events(world: &World, kind: &str) -> Vec<Value> {
	world
		.f
		.store
		.events(&TEST_FIXTURE, 0, Some(world.run.workspace_id), 1000)
		.await
		.unwrap()
		.into_iter()
		.filter(|event| event.kind == kind)
		.map(|event| event.data)
		.collect()
}

fn results(run: &Run) -> Vec<(String, Value)> {
	run.context
		.events()
		.filter_map(|event| match event {
			aidash_server::context::ContextEvent::Tool { call, result } => {
				Some((call.name.clone(), result.clone()))
			}
			_ => None,
		})
		.collect()
}

/// A crashed worker must also stop if a regression assertion unwinds the test.
struct WorkerTask(Option<tokio::task::JoinHandle<()>>);

impl WorkerTask {
	fn start(world: &World) -> Self {
		let harness = Harness {
			federation: world.f.clone(),
		};
		Self(Some(tokio::spawn(async move {
			loop {
				harness.worker_once().await.unwrap();
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})))
	}

	async fn stop(mut self) {
		let worker = self.0.take().unwrap();
		worker.abort();
		let error = worker.await.expect_err("the worker was interrupted");
		assert!(error.is_cancelled(), "worker failed: {error}");
	}
}

impl Drop for WorkerTask {
	fn drop(&mut self) {
		if let Some(worker) = &self.0 {
			worker.abort();
		}
	}
}

async fn wait_for_run(world: &World, mut done: impl FnMut(&Run) -> bool) -> Run {
	tokio::time::timeout(std::time::Duration::from_secs(20), async {
		loop {
			let run = world
				.f
				.store
				.run(&TEST_FIXTURE, world.run.id)
				.await
				.unwrap();
			assert!(run.error.is_none(), "worker failed: {:?}", run.error);
			if done(&run) {
				return run;
			}
			tokio::time::sleep(std::time::Duration::from_millis(20)).await;
		}
	})
	.await
	.expect("the worker persisted the expected state")
}

fn resumed_tool(run: &Run) -> &aidash_domain::run_state::ToolCallState {
	use aidash_domain::{RunState, WaitingState, run_state::ResumeState};
	match &run.state {
		RunState::ToolCall(tool) => tool,
		RunState::Waiting(wait) => match wait.as_ref() {
			WaitingState::Human {
				resume: ResumeState::ToolCall(tool),
				..
			}
			| WaitingState::Approval { resume: tool, .. }
			| WaitingState::Reconciliation { resume: tool, .. } => tool,
			other => panic!("expected a suspended tool call: {other:?}"),
		},
		other => panic!("expected a tool call: {other:?}"),
	}
}

async fn round_journal(world: &World) -> Vec<aidash_server::store::Invocation> {
	use aidash_server::apps::execution::models::Invocation;
	use reinhardt::db::orm::Model;
	world
		.f
		.store
		.pool
		.atomic(&TEST_FIXTURE, async |tx| {
			Ok(Invocation::objects()
				.all()
				.all_with_executor(tx)
				.await
				.map_err(reinhardt::core::exception::Error::from)?
				.into_iter()
				.filter(|row| row.run_id() == world.run.id && row.tool == "mcp.round")
				.map(Into::into)
				.collect())
		})
		.await
		.unwrap()
}

async fn expire_worker_lease_and_admitted_approval(world: &World, approval: uuid::Uuid) -> Value {
	use aidash_server::apps::execution::{capabilities::models::CoreRecords, models::Run};
	use reinhardt::db::orm::Model;
	world
		.f
		.store
		.pool
		.atomic(&TEST_FIXTURE, async |tx| {
			let mut record = Run::objects().get(world.run.id).get_with_db(tx).await?;
			assert!(
				record.lease_owner.is_some(),
				"live callbacks retain their worker lease"
			);
			record.lease_until = Some(chrono::Utc::now() - chrono::Duration::seconds(1));
			Run::objects().update_with_conn(tx, &record).await?;
			let mut admitted = CoreRecords::objects().get(approval).get_with_db(tx).await?;
			assert_eq!(admitted.kind, "tool_call");
			assert_eq!(admitted.state, "consumed");
			let consumed_at = admitted.data["consumed_at"].clone();
			assert!(consumed_at.is_string());
			admitted.expires_at = Some(chrono::Utc::now() - chrono::Duration::seconds(1));
			CoreRecords::objects()
				.update_with_conn(tx, &admitted)
				.await?;
			Ok(consumed_at)
		})
		.await
		.unwrap()
}

#[rstest::rstest]
#[tokio::test]
async fn a_run_pins_the_plugin_tools_asks_before_the_tool_that_is_not_read_only_and_closes_its_sessions(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: ApplicationFixture,
) {
	let world = world(fixture, false).await;
	let harness = Harness {
		federation: world.f.clone(),
	};

	let waiting = step_until(&harness, &world, |run| awaited_tool_call(run).is_some()).await;
	let pinned = events(&world, "run.plugin_tools_pinned").await;
	assert_eq!(pinned.len(), 1, "{pinned:?}");
	assert_eq!(pinned[0]["skipped"], json!([]));
	assert_eq!(
		world.mcp.requests("tools/list").len(),
		2,
		"the initial pin and the read-only call each verify the schema"
	);
	let tools = &waiting.context.plugin_tools.as_ref().unwrap().servers[0].tools;
	assert_eq!(
		tools
			.iter()
			.map(|tool| (tool.name.as_str(), tool.read_only))
			.collect::<Vec<_>>(),
		[("lookup", true), ("update", false)]
	);

	let (approval, call) = awaited_tool_call(&waiting).unwrap();
	assert_eq!(call.name, alias("update"));
	assert_eq!(
		world.mcp.calls(),
		[("lookup".to_owned(), json!({"q":"call 0"}))],
		"nothing else ran before the decision"
	);
	let (status, inbox) = request(
		&world.app,
		&world.token,
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
		.unwrap_or_else(|| panic!("the approval is not pending: {inbox}"));
	assert_eq!(card["kind"], "tool_call");
	assert_eq!(
		card["tool"],
		format!("agent-plugin:{PLUGIN}/{SERVER}/update")
	);
	assert_eq!(
		card["plugin_tool"],
		json!({
			"plugin": PLUGIN,
			"server": SERVER,
			"tool": "update",
			"digest": waiting.context.plugin_tools.as_ref().unwrap().servers[0].digest,
		})
	);
	assert_eq!(card["operation"], "invoke");
	assert_eq!(card["call"]["name"], alias("update"));
	assert_eq!(card["call"]["arguments"], json!({"q":"call 1"}));
	assert_eq!(card["viewer_is_approver"], true);

	let (status, decided) =
		decide_tool_call(&world.app, &world.token, approval, "allow_once").await;
	assert_eq!(status, 200, "{decided}");
	let finished = step_until(&harness, &world, |run| run.phase().as_str() == "COMPLETED").await;

	assert_eq!(
		world.mcp.calls(),
		[
			("lookup".to_owned(), json!({"q":"call 0"})),
			("update".to_owned(), json!({"q":"call 1"})),
		]
	);
	let recorded = results(&finished);
	assert_eq!(
		recorded
			.iter()
			.map(|(name, _)| name.as_str())
			.collect::<Vec<_>>(),
		[alias("lookup"), alias("update")]
	);
	for (name, result) in &recorded {
		assert_eq!(result["isError"], false, "{name}: {result}");
		assert!(result["error"].is_null(), "{name}: {result}");
	}
	assert_eq!(
		world.mcp.requests("tools/list").len(),
		3,
		"both calls verify their pinned schemas on the retained session"
	);
	assert_eq!(world.mcp.sessions_issued(), 1);
	let (_, all) = request(
		&world.app,
		&world.token,
		"GET",
		"/api/capabilities/approvals",
		Value::Null,
	)
	.await;
	assert_eq!(all["items"][0]["state"], "consumed", "{all}");
	for offered in world.model.offered.lock().unwrap().iter() {
		assert!(offered.contains(&alias("lookup")) && offered.contains(&alias("update")));
	}

	for _ in 0..50 {
		if world.mcp.ended().len() == 1 {
			break;
		}
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	assert_eq!(world.mcp.ended(), ["session-1"]);
	let (url, schema, f) = (world.url.clone(), world.schema.clone(), world.f.clone());
	drop(world);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_run_continues_without_the_tools_of_a_server_that_is_down_and_the_event_lists_it(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: ApplicationFixture,
) {
	let world = world(fixture, true).await;
	let harness = Harness {
		federation: world.f.clone(),
	};

	let finished = step_until(&harness, &world, |run| run.phase().as_str() == "COMPLETED").await;

	let pinned = events(&world, "run.plugin_tools_pinned").await;
	assert_eq!(pinned.len(), 1, "{pinned:?}");
	assert_eq!(
		pinned[0]["skipped"],
		json!([{"plugin": PLUGIN, "server": SERVER, "reason": "server_unavailable"}])
	);
	// No URL, header, detail or body reaches the event.
	let text = pinned[0].to_string();
	assert!(
		!text.contains("127.0.0.1") && !text.contains("unreachable"),
		"{text}"
	);
	let plugin_tools = finished.context.plugin_tools.as_ref().unwrap();
	assert!(plugin_tools.servers.is_empty());
	assert_eq!(plugin_tools.findings[0].finding.detail, "unreachable");
	for offered in world.model.offered.lock().unwrap().iter() {
		assert!(
			!offered.iter().any(|name| name.starts_with(PLUGIN)),
			"{offered:?}"
		);
	}
	assert!(results(&finished).is_empty());
	assert_eq!(world.mcp.sessions_issued(), 0);
	let (url, schema, f) = (world.url.clone(), world.schema.clone(), world.f.clone());
	drop(world);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[case::cancel(None, false, RunControlAction::Cancel)]
#[case::pause_store(None, false, RunControlAction::Pause)]
#[case::pause_subject_control(Some("control"), false, RunControlAction::Pause)]
#[case::pause_administrator_control(Some("control"), true, RunControlAction::Pause)]
#[case::pause_subject_management(Some("management"), false, RunControlAction::Pause)]
#[tokio::test]
async fn a_paused_or_cancelled_run_ends_its_retained_sessions_after_control_commits(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: ApplicationFixture,
	#[case] endpoint: Option<&str>,
	#[case] administrator: bool,
	#[case] action: RunControlAction,
) {
	let world = world(fixture, false).await;
	let harness = Harness {
		federation: world.f.clone(),
	};
	step_until(&harness, &world, |run| awaited_tool_call(run).is_some()).await;
	assert_eq!(world.mcp.sessions_issued(), 1);
	assert!(world.mcp.ended().is_empty());

	if let Some(endpoint) = endpoint {
		let token = if administrator {
			&world.f.config.api_token
		} else {
			&world.token
		};
		let (status, body) = request(
			&world.app,
			token,
			"POST",
			&format!("/api/runs/{}/{endpoint}", world.run.id),
			json!({"action":action}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	} else {
		world
			.f
			.store
			.control(&TEST_FIXTURE, world.run.id, action)
			.await
			.unwrap();
	}
	if action == RunControlAction::Cancel {
		step_until(&harness, &world, |run| run.phase().as_str() == "CANCELLED").await;
	}
	let controlled = world
		.f
		.store
		.run(&TEST_FIXTURE, world.run.id)
		.await
		.unwrap();
	assert_eq!(controlled.control, action.control());
	// A pause runs no worker: control must close the quiet retained session itself.
	for _ in 0..50 {
		if world.mcp.ended().len() == 1 {
			break;
		}
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	assert_eq!(world.mcp.ended(), ["session-1"]);
	assert_eq!(
		world.mcp.calls(),
		[("lookup".to_owned(), json!({"q":"call 0"}))],
		"the call that waited for its approval never ran"
	);
	let (url, schema, f) = (world.url.clone(), world.schema.clone(), world.f.clone());
	drop(world);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn modern_inputs_release_the_lease_resume_serially_and_charge_each_sampling_request_once(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: ApplicationFixture,
) {
	use aidash_domain::{RunState, WaitingState};
	let world = world(fixture, false).await;
	world.mcp.modern_inputs(json!({
        "a-form":{"method":"elicitation/create","params":{"mode":"form","message":"Choose a label","requestedSchema":{"type":"object","properties":{"label":{"type":"string","enum":["ok","no"]}},"required":["label"]}}},
        "b-sample":{"method":"sampling/createMessage","params":{"messages":[{"role":"user","content":{"type":"text","text":"Server sampling"}}],"systemPrompt":"sampling-only-system","maxTokens":32,"includeContext":"allServers"}},
        "c-sample":{"method":"sampling/createMessage","params":{"messages":[{"role":"user","content":{"type":"text","text":"Server sampling"}}],"systemPrompt":"sampling-only-system","maxTokens":32}}
    }));
	let harness = Harness {
		federation: world.f.clone(),
	};
	let waiting = step_until(&harness, &world, |run| matches!(&run.state, RunState::Waiting(wait) if matches!(wait.as_ref(), WaitingState::Human { .. }))).await;
	let RunState::Waiting(wait) = &waiting.state else {
		unreachable!()
	};
	let WaitingState::Human { request_id, .. } = wait.as_ref() else {
		unreachable!()
	};
	assert_eq!(world.mcp.calls().len(), 1);
	assert!(world.model.sampling.lock().unwrap().is_empty());
	let (status, value) = request(
		&world.app,
		&world.token,
		"POST",
		&format!("/api/human-requests/{request_id}/answer"),
		json!({"action":"accept","content":{"label":"ok"}}),
	)
	.await;
	assert_eq!(status, 200, "{value}");
	let waiting = step_until(&harness, &world, |run| awaited_tool_call(run).is_some()).await;
	let (sampling, _) = awaited_tool_call(&waiting).unwrap();
	let (_, inbox) = request(
		&world.app,
		&world.token,
		"GET",
		"/api/capabilities/approvals?state=pending&role=approver",
		Value::Null,
	)
	.await;
	let card = inbox["items"]
		.as_array()
		.unwrap()
		.iter()
		.find(|card| card["id"] == json!(sampling))
		.unwrap();
	assert_eq!(card["kind"], "sampling");
	assert_eq!(card["sampling"]["system_prompt"], "sampling-only-system");
	assert_eq!(card["sampling"]["max_tokens"], 32);
	assert_eq!(world.mcp.calls().len(), 1);
	let (status, value) = request(&world.app, &world.token, "POST", &format!("/api/capabilities/approvals/{sampling}/decide"), json!({"idempotency_key":uuid::Uuid::new_v4(),"expected_revision":card["revision"],"choice":"allow_run","targets":card["targets"],"expires_at":chrono::Utc::now()+chrono::Duration::minutes(10)})).await;
	assert_eq!(status, 200, "{value}");
	let after = step_until(&harness, &world, |run| {
		awaited_tool_call(run).is_some() && results(run).len() == 1
	})
	.await;
	assert_eq!(world.model.sampling.lock().unwrap().len(), 2);
	let calls = world.mcp.requests("tools/call");
	assert_eq!(calls.len(), 2);
	assert_eq!(calls[1]["arguments"], calls[0]["arguments"]);
	assert_eq!(
		calls[1]["inputResponses"]["a-form"]["content"]["label"],
		"ok"
	);
	assert_eq!(
		calls[1]["inputResponses"]["b-sample"]["content"]["text"],
		"sample completion"
	);
	assert_eq!(
		events(&world, "inference.accepted").await.len(),
		3,
		"ordinary model response and two separately journaled samples"
	);
	assert_eq!(results(&after).len(), 1);
	assert_eq!(world.mcp.sessions_issued(), 0);

	// A crash can leave the earlier input checkpoint after both accepted
	// sampling responses have committed. Reuse those responses even when the
	// approval later expires; neither the provider nor the wire call repeats.
	world
		.f
		.store
		.pool
		.atomic(&TEST_FIXTURE, async |tx| {
			use aidash_server::apps::execution::{
				capabilities::models::CoreRecords, models::Run as RunRecord,
			};
			use reinhardt::db::orm::Model;
			let mut approval = CoreRecords::objects().get(sampling).get_with_db(tx).await?;
			approval.expires_at = Some(chrono::Utc::now() - chrono::Duration::seconds(1));
			CoreRecords::objects()
				.update_with_conn(tx, &approval)
				.await?;
			let mut record = RunRecord::objects()
				.get(world.run.id)
				.get_with_db(tx)
				.await?;
			record.phase = serde_json::from_value(json!(waiting.phase()))?;
			record.pending = waiting.stored_pending().unwrap().into();
			record.context = serde_json::to_value(&waiting.context)?.into();
			record.lease_owner = None;
			record.lease_until = None;
			record.ledger_worker_ready = true;
			RunRecord::objects().update_with_conn(tx, &record).await?;
			Ok(())
		})
		.await
		.unwrap();
	let recovered = step_until(&harness, &world, |run| results(run).len() == 1).await;
	assert_eq!(results(&recovered), results(&after));
	assert_eq!(world.model.sampling.lock().unwrap().len(), 2);
	assert_eq!(world.mcp.requests("tools/call").len(), 2);
	assert_eq!(events(&world, "inference.accepted").await.len(), 3);
	let (url, schema, f) = (world.url.clone(), world.schema.clone(), world.f.clone());
	drop(world);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn new_user_input_during_a_modern_wait_does_not_discard_the_admitted_call(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: ApplicationFixture,
) {
	use aidash_domain::{RunState, WaitingState};
	let world = world(fixture, false).await;
	world.mcp.modern_inputs(json!({"form":{"method":"elicitation/create","params":{"message":"Continue?","requestedSchema":{"type":"object","properties":{}}}}}));
	let harness = Harness {
		federation: world.f.clone(),
	};
	let waiting = step_until(&harness, &world, |run| matches!(&run.state, RunState::Waiting(wait) if matches!(wait.as_ref(), WaitingState::Human { .. }))).await;
	let RunState::Waiting(wait) = &waiting.state else {
		unreachable!()
	};
	let WaitingState::Human { request_id, .. } = wait.as_ref() else {
		unreachable!()
	};
	let limit = world.f.run_message_limit(&waiting).await.unwrap();
	world
		.f
		.store
		.accept_run_message(
			&TEST_FIXTURE,
			waiting.id,
			"human",
			"Follow-up while waiting",
			"during-mcp-wait",
			limit,
		)
		.await
		.unwrap();
	let (status, response) = request(
		&world.app,
		&world.token,
		"POST",
		&format!("/api/human-requests/{request_id}/answer"),
		json!({"action":"decline"}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	let after = step_until(&harness, &world, |run| !results(run).is_empty()).await;
	let calls = world.mcp.requests("tools/call");
	assert_eq!(calls.len(), 2);
	assert_eq!(calls[1]["arguments"], calls[0]["arguments"]);
	assert_eq!(calls[1]["inputResponses"]["form"]["action"], "decline");
	assert_eq!(results(&after).len(), 1);
	let (url, schema, f) = (world.url.clone(), world.schema.clone(), world.f.clone());
	drop(world);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_modern_sampling_denial_finishes_the_logical_call_without_a_continuation_or_inference(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: ApplicationFixture,
) {
	let world = world(fixture, false).await;
	world.mcp.modern_inputs(json!({"sample":{"method":"sampling/createMessage","params":{"messages":[{"role":"user","content":{"type":"text","text":"Server sampling"}}],"systemPrompt":"sampling-only-system","maxTokens":32}}}));
	let harness = Harness {
		federation: world.f.clone(),
	};
	let waiting = step_until(&harness, &world, |run| awaited_tool_call(run).is_some()).await;
	let (approval, _) = awaited_tool_call(&waiting).unwrap();
	let (_, inbox) = request(
		&world.app,
		&world.token,
		"GET",
		"/api/capabilities/approvals?state=pending&role=approver",
		Value::Null,
	)
	.await;
	let card = inbox["items"]
		.as_array()
		.unwrap()
		.iter()
		.find(|item| item["id"] == json!(approval))
		.unwrap();
	assert_eq!(card["kind"], "sampling");
	let (status, value) = request(&world.app, &world.token, "POST", &format!("/api/capabilities/approvals/{approval}/decide"), json!({"idempotency_key":uuid::Uuid::new_v4(),"expected_revision":card["revision"],"choice":"deny"})).await;
	assert_eq!(status, 200, "{value}");
	let after = step_until(&harness, &world, |run| results(run).len() == 1).await;
	assert_eq!(results(&after)[0].1["error"], "MCP_SAMPLING_REFUSED");
	assert!(world.model.sampling.lock().unwrap().is_empty());
	assert_eq!(world.mcp.calls().len(), 1);
	let (url, schema, f) = (world.url.clone(), world.schema.clone(), world.f.clone());
	drop(world);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn state_only_modern_rounds_exhaust_the_continuation_limit_without_resetting_it(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: ApplicationFixture,
) {
	let world = world(fixture, false).await;
	world.mcp.modern_inputs(json!({}));
	let harness = Harness {
		federation: world.f.clone(),
	};
	let after = step_until(&harness, &world, |run| results(run).len() == 1).await;
	assert_eq!(results(&after)[0].1["error"], "MCP_CONTINUATION_LIMIT");
	assert_eq!(
		world.mcp.calls().len(),
		11,
		"initial request plus ten continuations"
	);
	assert_eq!(events(&world, "mcp.input_failed").await.len(), 1);
	let (url, schema, f) = (world.url.clone(), world.schema.clone(), world.f.clone());
	drop(world);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[case::live_form(false)]
#[case::live_sampling_during_modern_continuation(true)]
#[tokio::test]
async fn worker_loss_during_a_live_callback_reconciles_the_original_send_once(
	#[case] modern_sampling: bool,
	#[future(awt)]
	#[from(common::native_application)]
	fixture: ApplicationFixture,
) {
	use aidash_application::ports::tools::PluginSessions;
	use aidash_domain::{RunState, WaitingState};
	let world = world(fixture, false).await;
	world.model.only_update.store(true, SeqCst);
	let form = json!({"mode":"form","message":"Choose a label","requestedSchema":{"type":"object","properties":{"label":{"type":"string","enum":["ok","no"]}},"required":["label"]}});
	if modern_sampling {
		world
			.mcp
			.modern_inputs(json!({"first-form":{"method":"elicitation/create","params":form}}));
		world.mcp.live_input("sampling/createMessage", json!({"messages":[{"role":"user","content":{"type":"text","text":"Server sampling"}}],"systemPrompt":"sampling-only-system","maxTokens":32}));
	} else {
		world.mcp.live_input("elicitation/create", form);
	}
	let harness = Harness {
		federation: world.f.clone(),
	};
	let admission = step_until(&harness, &world, |run| awaited_tool_call(run).is_some()).await;
	let (tool_approval, original_call) = awaited_tool_call(&admission).unwrap();
	assert_eq!(original_call.name, alias("update"));
	assert_eq!(original_call.arguments, json!({"q":"call 0"}));
	assert!(world.mcp.calls().is_empty());
	let (status, response) =
		decide_tool_call(&world.app, &world.token, tool_approval, "allow_once").await;
	assert_eq!(status, 200, "{response}");

	if modern_sampling {
		let first = step_until(&harness, &world, |run| matches!(&run.state, RunState::Waiting(wait) if matches!(wait.as_ref(), WaitingState::Human { .. }))).await;
		let RunState::Waiting(wait) = first.state else {
			unreachable!()
		};
		let WaitingState::Human { request_id, .. } = wait.as_ref() else {
			unreachable!()
		};
		// This earlier wait must remain charged when a live callback replaces the input.
		tokio::time::sleep(std::time::Duration::from_millis(20)).await;
		let (status, response) = request(
			&world.app,
			&world.token,
			"POST",
			&format!("/api/human-requests/{request_id}/answer"),
			json!({"action":"accept","content":{"label":"ok"}}),
		)
		.await;
		assert_eq!(status, 200, "{response}");
	}

	// The callback travels over the live MCP stream. Observe the checkpoint that the
	// executing worker itself persisted, then lose that worker before the callback answer.
	let worker = WorkerTask::start(&world);
	let waiting = wait_for_run(&world, |run| run.lease_owner.is_some() && matches!(&run.state, RunState::Waiting(wait) if if modern_sampling { matches!(wait.as_ref(), WaitingState::Approval { .. }) } else { matches!(wait.as_ref(), WaitingState::Human { .. }) })).await;
	let before = resumed_tool(&waiting).mcp.as_ref().unwrap().clone();
	assert_eq!(resumed_tool(&waiting).response.tool_calls[0], original_call);
	assert_eq!(before.continuation_sends, u32::from(modern_sampling));
	if modern_sampling {
		assert!(
			before.waited_millis > 0,
			"the first form consumed human-wait budget"
		);
	}
	let send_count = if modern_sampling { 2 } else { 1 };
	assert_eq!(world.mcp.calls().len(), send_count);
	assert!(world.model.sampling.lock().unwrap().is_empty());
	let journal = round_journal(&world).await;
	let original_send = journal.iter().find(|row| row.status == "STARTED").unwrap();
	let original_key = original_send.idempotency_key.clone();
	let expected_input = json!({
		"call":{"id":"call-0","name":alias("update"),"arguments":{"q":"call 0"}},
		"request_state":if modern_sampling { json!("sealed-request-state") } else { Value::Null },
		"input_responses":if modern_sampling { json!({"first-form":{"action":"accept","content":{"label":"ok"}}}) } else { json!({}) }
	});
	assert_eq!(original_send.input, expected_input);
	worker.stop().await;
	world.f.store.plugin_sessions.close(world.run.id).await;
	let consumed_at = expire_worker_lease_and_admitted_approval(&world, tool_approval).await;

	if modern_sampling {
		let (approval, _) = awaited_tool_call(&waiting).unwrap();
		let (status, inbox) = request(
			&world.app,
			&world.token,
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
			.find(|card| card["id"] == json!(approval))
			.unwrap();
		assert_eq!(card["kind"], "sampling");
		let (status, response) = request(&world.app, &world.token, "POST", &format!("/api/capabilities/approvals/{approval}/decide"), json!({"idempotency_key":uuid::Uuid::new_v4(),"expected_revision":card["revision"],"choice":"allow_run","targets":card["targets"],"expires_at":chrono::Utc::now()+chrono::Duration::minutes(10)})).await;
		assert_eq!(status, 200, "{response}");
	} else {
		let RunState::Waiting(wait) = &waiting.state else {
			unreachable!()
		};
		let WaitingState::Human { request_id, .. } = wait.as_ref() else {
			unreachable!()
		};
		let (status, response) = request(
			&world.app,
			&world.token,
			"POST",
			&format!("/api/human-requests/{request_id}/answer"),
			json!({"action":"accept","content":{"label":"ok"}}),
		)
		.await;
		assert_eq!(status, 200, "{response}");
	}

	// Recovery cannot answer a dead wire callback by running it again. In particular,
	// granting Sampling after worker loss must not start a provider inference.
	let worker = WorkerTask::start(&world);
	let reconciliation = wait_for_run(&world, |run| {
		assert_eq!(world.mcp.calls().len(), send_count, "worker recovery sent an additional tools/call instead of reconciling the original send");
		assert!(world.model.sampling.lock().unwrap().is_empty(), "worker recovery inferred for the interrupted live Sampling callback");
		run.lease_owner.is_none() && matches!(&run.state, RunState::Waiting(wait) if matches!(wait.as_ref(), WaitingState::Reconciliation { .. }))
	}).await;
	worker.stop().await;
	let RunState::Waiting(wait) = &reconciliation.state else {
		unreachable!()
	};
	let WaitingState::Reconciliation {
		request_id,
		key,
		resume,
	} = wait.as_ref()
	else {
		unreachable!()
	};
	assert_eq!(key, &original_key);
	assert_eq!(resume.response.tool_calls[0], original_call);
	// The call was already admitted, so expiry while its worker was absent
	// cannot ask for or consume another approval during reconciliation.
	world
		.f
		.store
		.pool
		.atomic(&TEST_FIXTURE, async |tx| {
			use aidash_server::apps::execution::capabilities::models::CoreRecords;
			use reinhardt::db::orm::Model;
			let admitted = CoreRecords::objects()
				.get(tool_approval)
				.get_with_db(tx)
				.await?;
			assert_eq!(admitted.state, "consumed");
			assert_eq!(admitted.data["consumed_at"], consumed_at);
			Ok(())
		})
		.await
		.unwrap();
	assert!(
		waiting.stored_pending().unwrap()["requires"]
			.as_array()
			.unwrap()
			.contains(&json!("mcp_sent_round"))
	);
	let checkpoint = resume.mcp.as_ref().unwrap();
	assert_eq!(checkpoint.continuation_sends, before.continuation_sends);
	assert!(checkpoint.waited_millis >= before.waited_millis);
	let sent = checkpoint
		.sent
		.as_ref()
		.expect("the original sent identity is durable");
	assert_eq!(sent.key, original_key);
	assert_eq!(sent.input, expected_input);
	let journal = round_journal(&world).await;
	assert_eq!(journal.len(), send_count);
	assert_eq!(
		journal
			.iter()
			.find(|row| row.idempotency_key == original_key)
			.unwrap()
			.status,
		"UNCERTAIN"
	);

	let confirmed = json!({"effect":"verified original send"});
	let (status, response) = request(
		&world.app,
		&world.token,
		"POST",
		&format!("/api/human-requests/{request_id}/answer"),
		json!({"result":confirmed}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	// Commit reconciliation, then stop at the separate durable step before the
	// logical tool result is consumed. An empty recovery pass may first wrap
	// its keyset cursor; the next worker must still adopt the result once.
	let committed = step_until(&harness, &world, |run| {
		matches!(run.state, RunState::ToolCall(_))
	})
	.await;
	assert!(results(&committed).is_empty());
	let journal = round_journal(&world).await;
	let completed = journal
		.iter()
		.find(|row| row.idempotency_key == original_key)
		.unwrap();
	assert_eq!(completed.status, "COMPLETED");
	assert_eq!(completed.input, expected_input);
	assert_eq!(
		completed.result,
		Some(json!({"outcome":"complete","data":confirmed}))
	);
	let after = step_until(&harness, &world, |run| results(run).len() == 1).await;
	assert_eq!(results(&after), vec![(alias("update"), confirmed)]);
	let finished = step_until(&harness, &world, |run| run.phase().as_str() == "COMPLETED").await;
	assert_eq!(results(&finished), results(&after));
	assert_eq!(world.mcp.calls().len(), send_count);
	assert!(world.model.sampling.lock().unwrap().is_empty());
	assert_eq!(
		events(&world, "tool.completed")
			.await
			.iter()
			.filter(|event| event["idempotency_key"] == original_key)
			.count(),
		1
	);
	assert_eq!(round_journal(&world).await.len(), send_count);
	let (url, schema, f) = (world.url.clone(), world.schema.clone(), world.f.clone());
	drop(world);
	cleanup(f, &url, &schema).await;
}
