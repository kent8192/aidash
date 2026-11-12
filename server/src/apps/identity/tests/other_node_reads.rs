#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
//! Reads that depend on another Node of the same Tenant and Deployment (#751).
//!
//! A Run on the default Node discovers an Agent that only the second Node
//! offers, creates a Task, and hands it over. Since #769 a read of what that Run
//! produced is decided on this Node, by `registry.read` and `federation.discover`,
//! without asking the second Node: its silence or its deletion hides nothing.
//! Delegation and admission still reach that Node.
use aidash_application::tenancy::TEST_FIXTURE;
use common::upstream_fixtures;
use futures_util::{FutureExt, future::BoxFuture};
use std::{
	sync::{Arc, Mutex},
	time::Duration,
};
use upstream_fixtures::{async_upstream, reply};
#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_domain::nodes::{NodeAddress, NodeName};
use aidash_server::{
	domain::{Run, Task},
	federation::Federation,
	harness::Harness,
};
use common::*;
use serde_json::{Value, json};
use uuid::Uuid;

/// What the scripted model does, decided from the transcript it is sent so
/// that the coordinator and the receiver can share one endpoint.
struct Script {
	/// The address of the second Node, once it exists.
	sibling: Mutex<Option<String>>,
	/// Whether the coordinator hands the Task it created to the second Node.
	delegate: Mutex<bool>,
	/// What the Task the coordinator creates requires.
	requirements: Mutex<Value>,
	/// How many requests the coordinator has made.
	stage: std::sync::atomic::AtomicUsize,
	/// How many Tasks the coordinator creates before it finishes or delegates.
	tasks: std::sync::atomic::AtomicUsize,
	/// Wait for the delegated Run and read its message before creating more Tasks.
	read_delegated: std::sync::atomic::AtomicBool,
	child_completed: std::sync::atomic::AtomicBool,
	result_message: Mutex<Option<Uuid>>,
}

#[rstest::fixture]
fn script() -> Arc<Script> {
	Arc::new(Script {
		sibling: Mutex::new(None),
		delegate: Mutex::new(true),
		requirements: Mutex::new(json!({})),
		stage: Default::default(),
		tasks: std::sync::atomic::AtomicUsize::new(1),
		read_delegated: std::sync::atomic::AtomicBool::new(false),
		child_completed: std::sync::atomic::AtomicBool::new(false),
		result_message: Mutex::new(None),
	})
}

fn tool_call(id: &str, name: &str, arguments: Value) -> Value {
	json!({"role":"assistant","content":null,"tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]})
}

/// The coordinator's task text; the delegated Task carries other text.
const COORDINATOR_TASK: &str = "Use approved tools";
const HANDED_OVER: &str = "Handed over to the second Node";
/// Discover, create, then finish: the third request follows the saved creation.
const WITHOUT_DELEGATION: usize = 3;
/// Discover, create, delegate, then finish.
const WITH_DELEGATION: usize = 4;

/// The first UUID within `window` bytes after the last `anchor` in `text`.
fn uuid_after(text: &str, anchor: &str, window: usize) -> Option<String> {
	let at = text.rfind(anchor)?;
	let to = (at + anchor.len() + window).min(text.len());
	let slice = text.get(at..to)?;
	let bytes = slice.as_bytes();
	(0..bytes.len().saturating_sub(35)).find_map(|start| {
		let candidate = slice.get(start..start + 36)?;
		let shaped = candidate.char_indices().all(|(index, character)| {
			if matches!(index, 8 | 13 | 18 | 23) {
				character == '-'
			} else {
				character.is_ascii_hexdigit()
			}
		});
		shaped.then(|| candidate.to_owned())
	})
}

fn next_message(script: &Script, request: &Value) -> Value {
	// The history travels inside the prompt, so the coordinator's progress is
	// counted here and the Task it created is read out of the transcript.
	let transcript = request["messages"].to_string();
	if script
		.read_delegated
		.load(std::sync::atomic::Ordering::SeqCst)
		&& request["messages"].as_array().is_some_and(|messages| {
			messages
				.iter()
				.filter_map(|message| message["content"].as_str())
				.filter_map(|content| serde_json::from_str::<Value>(content).ok())
				.any(|context| {
					context
						.pointer("/current/identity/agent_id")
						.and_then(Value::as_str)
						== Some("sibling-agent")
				})
		}) {
		return json!({"role":"assistant","content":"Done"});
	}
	if !transcript.contains(COORDINATOR_TASK) {
		// The receiver on the second Node.
		return json!({"role":"assistant","content":"Done"});
	}
	let created = script.tasks.load(std::sync::atomic::Ordering::SeqCst);
	let read_delegated = script
		.read_delegated
		.load(std::sync::atomic::Ordering::SeqCst);
	match script
		.stage
		.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
	{
		0 => tool_call(
			"discover",
			"agent_discover",
			json!({"capability":"sibling.discovery"}),
		),
		// With delegation the first Task is handed over before the others are
		// created, so those others follow the delegated Run's results.
		stage
			if (1..=created).contains(&stage)
				&& !(*script.delegate.lock().unwrap() && stage > 1) =>
		{
			tool_call(
				&format!("create-{stage}"),
				"task_create",
				json!({
					"title":"Hand over",
					"description":HANDED_OVER,
					"requirements":script.requirements.lock().unwrap().clone(),
				}),
			)
		}
		2 if read_delegated => tool_call("wait-created", "workspace_wait", json!({"seconds":1})),
		stage
			if stage == (if read_delegated { 3 } else { 2 })
				&& *script.delegate.lock().unwrap() =>
		{
			let task = uuid_after(&transcript, HANDED_OVER, 300)
				.expect("the created Task's id is in the transcript");
			tool_call(
				"delegate",
				"task_delegate",
				json!({
					"task_id": task,
					"node_id": script.sibling.lock().unwrap().clone().expect("the second Node exists"),
					"agent": {"id":"sibling-agent","version":"1.0.0"},
				}),
			)
		}
		4 if read_delegated => tool_call("wait-child", "workspace_wait", json!({"seconds":1})),
		5 if read_delegated
			&& !script
				.child_completed
				.load(std::sync::atomic::Ordering::SeqCst) =>
		{
			script
				.stage
				.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
			tool_call("wait-again", "workspace_wait", json!({"seconds":1}))
		}
		5 if read_delegated => {
			let message = script
				.result_message
				.lock()
				.unwrap()
				.expect("the receiver's message exists");
			tool_call(
				"read-child",
				"workspace_read",
				json!({"kind":"message", "id":message}),
			)
		}

		stage
			if *script.delegate.lock().unwrap()
				&& ((if read_delegated { 6 } else { 3 })
					..=created + if read_delegated { 4 } else { 1 })
					.contains(&stage) =>
		{
			tool_call(
				&format!("create-{stage}"),
				"task_create",
				json!({"title":"Hand over","description":HANDED_OVER,"requirements":{}}),
			)
		}
		_ => json!({"role":"assistant","content":"Completed"}),
	}
}

#[rstest::fixture]
fn router(#[from(script)] script: Arc<Script>) -> upstream_fixtures::RouterFuture {
	async move {
		let server = reinhardt::test::stub::StubRouter::new()
			.route(
				"/v1/chat/completions",
				http::Method::POST,
				reply(move |request: reinhardt::Request| {
					let script = script.clone();
					async move {
						let body = request.json::<Value>().unwrap();
						let message = next_message(&script, &body);
						let reason = if message.get("tool_calls").is_some() {
							"tool_calls"
						} else {
							"stop"
						};
						reinhardt::Response::ok()
							.with_json(&json!({"choices":[{"index":0,"finish_reason":reason,"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
							.unwrap()
					}
				}),
			)
			 .route(
                "/v1/embeddings", http::Method::POST,
                reply(|_request: reinhardt::Request| async move {
                    reinhardt::Response::ok().with_json(&json!({"model":"home-vector","data":[{"index":0,"embedding":[1.0,0.0,0.0]}],"usage":{"prompt_tokens":1,"total_tokens":1}})).unwrap()
                }),
            )
            .into_server_router();
		Arc::new(server)
	}
	.boxed()
	.shared()
}

struct Provider {
	application: ApplicationFixture,
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	script: Arc<Script>,
}

#[rstest::fixture]
fn provider(
	#[from(script)] script: Arc<Script>,
	#[from(common::runtime)] _runtime: common::RuntimeFuture,
	#[from(router)]
	#[with(script.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[from(async_upstream)]
	#[with(_router.clone())]
	_server: upstream_fixtures::UpstreamFuture,
	#[from(common::native_application)]
	#[with(Default::default(),aidash_server::sse::Service::new(Default::default()),Arc::new(|r|r),_runtime.clone())]
	_application: common::ApplicationFuture,
) -> BoxFuture<'static, Provider> {
	async move {
		Provider {
			application: _application.await,
			server: _server.await,
			script,
		}
	}
	.boxed()
}

/// Two Nodes of the Tenant `acme`: the default one, where Alice's Run works,
/// and a second one created through the Tenant's administration route.
struct World {
	f: Federation,
	app: TestApplication,
	token: String,
	task: Uuid,
	sibling: NodeAddress,
	script: Arc<Script>,
	/// Keeps the model endpoint and the disposable database alive.
	model_server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	_runtime: RuntimeFixture,
	url: String,
	schema: String,
}

async fn world(provider: Provider) -> World {
	let (f, url, schema) = provider.application.runtime.parts();
	let runtime = provider.application.runtime.clone();
	let app = provider.application.application;
	let endpoint = provider.server.url.clone();
	let semantic = provider
		.script
		.read_delegated
		.load(std::sync::atomic::Ordering::SeqCst);
	let (_policy, token, task) = bootstrap_with_context(&f, &app, &endpoint, semantic).await;
	if semantic {
		f.store.keyless.permit_origin_of(&endpoint).unwrap();
		f.store.keyless.declare_hosted_origin_of(&endpoint).unwrap();
		let embedding = json!({"provider":"openai","endpoint":format!("{endpoint}/v1"),"model":"home-vector","model_version":"1","dimensions":3});
		let (status, body) = request(&app, &token, "POST", "/api/registry", json!({"id":"home-embedding","version":"1.0.0","kind":"embedding","name":{"en":"Home embedding"},"description":{"en":"Deterministic semantic fixture"},"config":embedding})).await;
		assert_eq!(status, 200, "{body}");
		let (status, body) = request(&app, &token, "POST", "/api/authorization/acme/catalog", json!({"node":NodeName::derived("acme"),"entry":{"id":"home-embedding","version":"1.0.0"},"expected_revision":0,"enabled":true})).await;
		assert_eq!(status, 200, "{body}");
		let workspace = f
			.store
			.task(&TEST_FIXTURE, task)
			.await
			.unwrap()
			.workspace_id;
		let (status, body) = request(&app, &common::tenant_token(&f.store, "acme").await, "POST", &format!("/api/workspaces/{workspace}/semantic/index"), json!({"expected_revision":0,"spec":{"embedding":embedding,"vector":{"provider":"postgres","endpoint":"local","credential_env":null},"enabled":true,"auto_context":true,"max_sources":100,"max_results":10,"max_result_tokens":4096,"max_input_bytes":32768}})).await;
		assert_eq!(status, 200, "{body}");
	}
	// The second Node comes from the real route, as a Tenant Administrator uses it.
	let (status, created) = administer_node(
		&f,
		"acme",
		"/api/tenants/acme/nodes",
		json!({"label":"Sibling"}),
	)
	.await;
	assert_eq!(status, 201, "{created}");
	let sibling = f
		.config
		.deployment
		.node(NodeName::parse(created["name"].as_str().unwrap()).unwrap());
	*provider.script.sibling.lock().unwrap() = Some(sibling.to_string());
	if semantic {
		// The receiver rechecks the entire executor chain for a RequiredHome Run.
		let (status, current) = request(
			&app,
			&f.config.api_token,
			"GET",
			"/api/authorization/acme",
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{current}");
		let mut bundle = current["bundle"].clone();
		bundle["subjects"]
			[aidash_domain::agent_subject(&sibling.to_string(), "sibling-agent", "1.0.0")] =
			json!({"kind":"agent", "node": sibling.node});
		let (status, body) = request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":current["revision"],"bundle":bundle}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	}

	// An Agent that only the second Node offers. The Tenant approves it, and
	// everything it depends on, there.
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let mut entry = f.registry.get(&tenant, "research", "1.0.0").await.unwrap();
	entry.id = "sibling-agent".into();
	entry.capabilities = vec!["sibling.discovery".into()];
	entry.binding_normalization = None;
	f.registry.register(&tenant, entry.clone()).await.unwrap();
	approve_agent_on(&f, "acme", &entry, &sibling).await;
	World {
		f,
		app,
		token,
		task,
		sibling,
		script: provider.script,
		model_server: provider.server,
		_runtime: runtime,
		url,
		schema,
	}
}

impl World {
	async fn get(&self, path: &str) -> (u16, Value) {
		common::get(&self.app, &self.token, path).await
	}

	/// Alice claims her Task with `research`, whose Run follows the script. The
	/// worker steps until the coordinator has made `requests` model requests,
	/// which is when what it did before has been saved. A Run with an open
	/// child Task does not complete, so completion is not awaited.
	async fn run_coordinator(&self, requests: usize) -> Run {
		let (status, claimed) = common::post(
			&self.app,
			&self.token,
			&format!("/api/tasks/{}/claim", self.task),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
		)
		.await;
		assert_eq!(status, 200, "{claimed}");
		self.advance_coordinator(requests).await
	}

	async fn advance_coordinator(&self, requests: usize) -> Run {
		let worker = Harness {
			federation: self.f.clone(),
		};
		tokio::time::timeout(Duration::from_secs(120), async {
			loop {
				worker.worker_once().await.unwrap();
				let run = self.coordinator().await;
				assert!(
					run.error.is_none(),
					"the coordinator stopped: {:?} {}",
					run.error,
					serde_json::to_string(&run.recovery).unwrap()
				);
				if self.script.stage.load(std::sync::atomic::Ordering::SeqCst) >= requests {
					return run;
				}
				tokio::time::sleep(Duration::from_millis(20)).await;
			}
		})
		.await
		.expect("the coordinator makes its requests")
	}

	async fn coordinator(&self) -> Run {
		self.f
			.store
			.runs(&TEST_FIXTURE)
			.await
			.unwrap()
			.into_iter()
			.find(|run| run.agent_id == "research")
			.expect("the coordinator's Run exists")
	}

	/// Every Task the coordinator created.
	async fn all_handed_over(&self) -> Vec<Task> {
		let workspace = self.f.store.task(&TEST_FIXTURE, self.task).await.unwrap();
		self.f
			.store
			.tasks(&TEST_FIXTURE, Some(workspace.workspace_id))
			.await
			.unwrap()
			.into_iter()
			.filter(|task| task.title == "Hand over")
			.collect()
	}

	/// The Task the coordinator created.
	async fn handed_over(&self) -> Task {
		let workspace = self.f.store.task(&TEST_FIXTURE, self.task).await.unwrap();
		self.f
			.store
			.tasks(&TEST_FIXTURE, Some(workspace.workspace_id))
			.await
			.unwrap()
			.into_iter()
			.find(|task| task.title == "Hand over")
			.expect("the coordinator created a Task")
	}

	/// The Run on the second Node, once the worker has finished it.
	async fn receiver(&self) -> Run {
		let worker = Harness {
			federation: self.f.clone(),
		};
		tokio::time::timeout(Duration::from_secs(120), async {
			loop {
				worker.worker_once().await.unwrap();
				let found = self
					.f
					.store
					.runs(&TEST_FIXTURE)
					.await
					.unwrap()
					.into_iter()
					.find(|run| run.agent_id == "sibling-agent");
				if let Some(run) = found
					&& matches!(run.phase().as_str(), "COMPLETED" | "FAILED")
				{
					return run;
				}
				tokio::time::sleep(Duration::from_millis(20)).await;
			}
		})
		.await
		.expect("the receiver's Run finishes")
	}

	async fn close(self) {
		cleanup(self.f, &self.url, &self.schema).await;
	}
}

/// Whether the listing of Tasks holds `task`.
fn listed(tasks: &Value, task: Uuid) -> bool {
	tasks["tasks"]
		.as_array()
		.unwrap_or_else(|| panic!("a list of Tasks: {tasks}"))
		.iter()
		.any(|item| item["id"] == json!(task))
}

/// Adds `subjects` and `policies` to the Tenant's policy bundle.
async fn add_policies(world: &World, subjects: &[&str], policies: Vec<Value>) {
	let (status, current) = common::get(
		&world.app,
		&world.f.config.api_token,
		"/api/authorization/acme",
	)
	.await;
	assert_eq!(status, 200, "{current}");
	let mut bundle = current["bundle"].clone();
	for subject in subjects {
		bundle["subjects"][*subject] = json!({"kind":"user"});
	}
	bundle["policies"].as_array_mut().unwrap().extend(policies);
	let (status, body) = common::post(
		&world.app,
		&world.f.config.api_token,
		"/api/authorization/acme",
		json!({"expected_revision":current["revision"],"bundle":bundle}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
}

/// Bob may read the Workspace but may not discover the second Node.
fn bob_cannot_discover() -> Value {
	json!({"id":"bob-cannot-discover","effect":"deny","subjects":{"ids":["bob"]},
		"actions":["federation.discover"],"resources":{"kinds":["node"]}})
}

/// Carol may not read Workspaces.
fn carol_reads_nothing() -> Value {
	json!({"id":"carol-reads-nothing","effect":"deny","subjects":{"ids":["carol"]},
		"actions":["workspace.read"],"resources":{"kinds":["*"]}})
}

/// The receiver's Run is admitted and starts on the second Node, then stops
/// for a reason the system states. A Run admitted by a Remote Grant is judged
/// at the strictest Data Protection Level, because the receipt does not publish
/// the source Tenant's current policy, and the fixture's Keyless model endpoint
/// cannot meet that level. The refusal is legitimate; delegation itself worked
/// (#751, item 17).
fn stops_at_data_protection(receiver: &Run) {
	assert_eq!(receiver.phase().as_str(), "FAILED", "{:?}", receiver.error);
	assert!(
		receiver
			.error
			.as_deref()
			.is_some_and(|error| error.contains("Data Protection Level ZeroDataRetention")),
		"{:?}",
		receiver.error
	);
}

/// How many Agents of the second Node the Run recorded as read dependencies.
async fn recorded_remote_reads(f: &Federation, run: Uuid) -> i64 {
	sqlx::query_scalar("SELECT COUNT(*) FROM authorization_run_remote_reads WHERE run_id = $1")
		.bind(run)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap()
}

/// A Task whose producer discovered an Agent on another Node of the Tenant is
/// readable by the person who owns it, and a Run that did not discover is too.
/// Before the Store held the in-process handlers both Task reads answered 403.
#[rstest::rstest]
#[tokio::test]
async fn outputs_of_a_run_that_discovered_an_agent_on_another_node_are_readable(
	#[future(awt)] provider: Provider,
) {
	provider.script.delegate.lock().unwrap().clone_from(&false);
	let world = world(provider).await;
	let coordinator = world.run_coordinator(WITHOUT_DELEGATION).await;
	assert!(
		recorded_remote_reads(&world.f, coordinator.id).await > 0,
		"the Run recorded its discovery on the second Node"
	);
	let child = world.handed_over().await;
	for path in [
		format!("/api/tasks/{}/runs", child.id),
		format!("/api/runs/{}", coordinator.id),
	] {
		let (status, body) = world.get(&path).await;
		assert_eq!(status, 200, "{path}: {body}");
	}
	// The listing includes the Task the Run created.
	let (status, tasks) = world.get("/api/tasks").await;
	assert_eq!(status, 200, "{tasks}");
	assert!(listed(&tasks, child.id), "{tasks}");
	// The Task Alice created herself has no such producer.
	let (status, body) = world.get(&format!("/api/tasks/{}/runs", world.task)).await;
	assert_eq!(status, 200, "{body}");
	world.close().await;
}

/// Withdrawing the Agent's approval on the other Node governs its use, not reads
/// of what a Run already produced with it, so the reads stay 200 (#769).
#[rstest::rstest]
#[tokio::test]
async fn a_withdrawn_approval_on_the_other_node_hides_nothing(#[future(awt)] provider: Provider) {
	*provider.script.delegate.lock().unwrap() = false;
	let world = world(provider).await;
	let coordinator = world.run_coordinator(WITHOUT_DELEGATION).await;
	let child = world.handed_over().await;
	let path = format!("/api/tasks/{}/runs", child.id);
	let run = format!("/api/runs/{}", coordinator.id);
	let (status, body) = world.get(&path).await;
	assert_eq!(status, 200, "{body}");
	let authorization = aidash_server::authorization::Authorization {
		pool: world.f.store.pool.clone(),
	};
	let reference = aidash_domain::registry::EntityRef {
		id: "sibling-agent".into(),
		version: "1.0.0".into(),
	};
	let binding = authorization
		.catalog("acme", &world.sibling.node)
		.await
		.unwrap()
		.into_iter()
		.find(|binding| binding.entry_id == reference.id)
		.expect("the approval exists");
	authorization
		.set_catalog(
			"acme",
			&world.sibling.node,
			&reference,
			binding.revision,
			false,
			"deployment_administrator",
		)
		.await
		.unwrap();
	for path in [&path, &run] {
		let (status, body) = world.get(path).await;
		assert_eq!(status, 200, "{path}: {body}");
	}
	world.close().await;
}

/// A person's `POST /api/tasks/{id}/delegate` to the other Node creates the
/// Remote Grant, and the Run starts and finishes there.
#[rstest::rstest]
#[tokio::test]
async fn a_person_delegates_a_task_whose_producer_discovered_the_agent(
	#[future(awt)] provider: Provider,
) {
	*provider.script.delegate.lock().unwrap() = false;
	let world = world(provider).await;
	world.run_coordinator(WITHOUT_DELEGATION).await;
	let child = world.handed_over().await;
	let (status, body) = common::post(
		&world.app,
		&world.token,
		&format!("/api/tasks/{}/delegate", child.id),
		json!({"node_id":world.sibling.to_string(),"agent":{"id":"sibling-agent","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let grants: i64 = sqlx::query_scalar(
		"SELECT COUNT(*) FROM authorization_remote_grants WHERE task_id = $1 AND node_id = $2",
	)
	.bind(child.id)
	.bind(world.sibling.to_string())
	.fetch_one(world.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(grants, 1);
	let receiver = world.receiver().await;
	stops_at_data_protection(&receiver);
	world.close().await;
}

/// The Agent hands the Task over itself, `task_delegate` runs through to a
/// completed Run on the receiver, and the Run that delegated stays readable.
#[rstest::rstest]
#[tokio::test]
async fn an_agent_delegates_to_the_node_it_discovered_the_agent_on(
	#[future(awt)] provider: Provider,
) {
	let world = world(provider).await;
	let coordinator = world.run_coordinator(WITH_DELEGATION).await;
	let (status, details) = world.get(&format!("/api/runs/{}", coordinator.id)).await;
	assert_eq!(status, 200, "{details}");
	let delegation = details["invocations"]
		.as_array()
		.unwrap()
		.iter()
		.find(|invocation| invocation["tool"] == "task_delegate")
		.unwrap_or_else(|| panic!("the Run called task_delegate: {details}"));
	assert_eq!(delegation["status"], "COMPLETED", "{delegation}");
	assert!(
		delegation["result"].get("error").is_none(),
		"the delegation was refused: {delegation}"
	);
	let receiver = world.receiver().await;
	stops_at_data_protection(&receiver);
	// The Run that delegated, and what it produced, read the same way at Home.
	let child = world.handed_over().await;
	for path in [
		format!("/api/tasks/{}/runs", child.id),
		format!("/api/runs/{}", coordinator.id),
	] {
		let (status, body) = world.get(&path).await;
		assert_eq!(status, 200, "{path}: {body}");
	}
	// The Run on the other Node is a remote admission of the Home's grant. Its
	// own view is a separate question from the outputs of the Run that
	// delegated: the person is refused it today, with no Node blamed.
	let (status, body) = world.get(&format!("/api/runs/{}", receiver.id)).await;
	assert_eq!(status, 403, "{body}");
	assert_ne!(body["code"], "node_unreachable", "{body}");
	world.close().await;
}

/// Serves the application from a pool of `connections`, rebuilding it. With one
/// connection a reader holds the pool for its own authority, so the peer it
/// asks cannot get a connection within a second and does not answer. The
/// original pool is returned so that peers can answer again.
async fn limit_pool(world: &mut World, connections: u32) -> aidash_server::database::native::Pool {
	limit_pool_waiting(world, connections, Duration::from_secs(1)).await
}

/// [`limit_pool`] where a connection is waited for up to `acquire`.
async fn limit_pool_waiting(
	world: &mut World,
	connections: u32,
	acquire: Duration,
) -> aidash_server::database::native::Pool {
	let small = world
		.f
		.store
		.pool
		.options()
		.clone()
		.max_connections(connections)
		.min_connections(0)
		.acquire_timeout(acquire)
		.connect_with(world.f.store.pool.connect_options().as_ref().clone())
		.await
		.unwrap();
	let original = std::mem::replace(&mut world.f.store.pool, small.into());
	world.app = common::application_with_settings(
		world.f.clone(),
		aidash_server::http::Settings {
			auth_burst: 10000,
			..Default::default()
		},
	)
	.await;
	original
}

/// A second Node that does not answer leaves a read of what a Run found there
/// decided here (#769): the single read and the list answer, and a reader who may
/// not discover that Node is still refused, naming nothing.
#[rstest::rstest]
#[tokio::test]
async fn a_silent_node_of_the_tenant_leaves_the_read_decided_here(
	#[future(awt)] provider: Provider,
) {
	*provider.script.delegate.lock().unwrap() = false;
	let mut world = world(provider).await;
	world.run_coordinator(WITHOUT_DELEGATION).await;
	let child = world.handed_over().await;
	add_policies(&world, &["bob"], vec![bob_cannot_discover()]).await;
	let bob = subject_credential(&world.f.store, "acme", "bob").await;
	let single = format!("/api/tasks/{}/runs", child.id);

	let original = limit_pool(&mut world, 1).await;
	let (status, body) = world.get(&single).await;
	assert_eq!(status, 200, "{body}");
	let (status, tasks) = world.get("/api/tasks").await;
	assert_eq!(status, 200, "{tasks}");
	assert!(listed(&tasks, child.id), "{tasks}");
	let (status, body) = common::get(&world.app, &bob, &single).await;
	assert_eq!(status, 403, "{body}");
	assert_ne!(body["code"], "node_unreachable", "{body}");
	restore_pool(&mut world, original).await;

	let (status, body) = world.get(&single).await;
	assert_eq!(status, 200, "{body}");
	world.close().await;
}

/// With a pool too small for the nested checks, every read still ends, none
/// hangs, and the pool serves the next read (#751, item 12). The pool gives up
/// after 3 s here, before the 10 s limit on a check can matter; that limit is
/// fixed by `a_check_ends_at_the_ten_second_limit_not_at_the_pools`.
#[rstest::rstest]
#[tokio::test]
async fn reads_on_a_saturated_pool_end_within_the_limit(#[future(awt)] provider: Provider) {
	*provider.script.delegate.lock().unwrap() = false;
	let mut world = world(provider).await;
	world.run_coordinator(WITHOUT_DELEGATION).await;
	let child = world.handed_over().await;
	// Two connections for four readers: each holds one for its own
	// authority, so the checks they open compete for the rest. The Registry
	// the in-process handlers hold keeps the original pool.
	let small = world
		.f
		.store
		.pool
		.options()
		.clone()
		.max_connections(2)
		.min_connections(0)
		.acquire_timeout(Duration::from_secs(3))
		.connect_with(world.f.store.pool.connect_options().as_ref().clone())
		.await
		.unwrap();
	world.f.store.pool = small.clone().into();
	world.app = common::application_with_settings(
		world.f.clone(),
		aidash_server::http::Settings {
			auth_burst: 10000,
			..Default::default()
		},
	)
	.await;
	let path = format!("/api/tasks/{}/runs", child.id);
	let started = std::time::Instant::now();
	let answers = tokio::time::timeout(
		Duration::from_secs(60),
		futures_util::future::join_all((0..4).map(|_| world.get(&path))),
	)
	.await
	.expect("reads must not hang on pool capacity");
	for (status, body) in &answers {
		assert!(
			matches!(*status, 200 | 500 | 503),
			"a read ends with an answer or an error, never silence: {status} {body}"
		);
	}
	assert!(started.elapsed() < Duration::from_secs(60));
	// The pool recovers: once the abandoned checks release their connections, a
	// lone read confirms again.
	let recovering = std::time::Instant::now();
	let mut recovered = (0, Value::Null);
	while recovering.elapsed() < Duration::from_secs(30) {
		recovered = world.get(&path).await;
		if recovered.0 == 200 {
			break;
		}
		tokio::time::sleep(Duration::from_secs(1)).await;
	}
	assert_eq!(recovered.0, 200, "{}", recovered.1);
	let probe: i32 = sqlx::query_scalar("SELECT 1")
		.fetch_one(&small)
		.await
		.unwrap();
	assert_eq!(probe, 1);
	world.close().await;
}

/// What a Tenant Administrator gets once the Node a Run discovered an Agent on
/// is deleted (#751, item 18). Since #769 the read is decided here, so deleting
/// that Node hides nothing.
#[rstest::rstest]
#[tokio::test]
async fn deleting_the_discovered_node_hides_nothing(#[future(awt)] provider: Provider) {
	*provider.script.delegate.lock().unwrap() = false;
	let world = world(provider).await;
	world.run_coordinator(WITHOUT_DELEGATION).await;
	let child = world.handed_over().await;
	let path = format!("/api/tasks/{}/runs", child.id);
	let (status, body) = world.get(&path).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = administer_node(
		&world.f,
		"acme",
		&format!("/api/tenants/acme/nodes/{}/deletion", world.sibling.node),
		json!({"confirmation":world.sibling.node}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = world.get(&path).await;
	assert_eq!(status, 200, "{body}");
	let (status, tasks) = world.get("/api/tasks").await;
	assert_eq!(status, 200, "{tasks}");
	assert!(listed(&tasks, child.id), "{tasks}");
	world.close().await;
}

/// Opening a Run's live inference stream while the second Node is silent: what the
/// Run read there is decided here (#769), so the stream opens. A reader who may not
/// discover that Node, and one who may not read Workspaces, stay `403` naming no Node.
#[rstest::rstest]
#[tokio::test]
async fn opening_a_run_stream_while_a_node_is_silent_opens_and_a_refusal_stays(
	#[future(awt)] provider: Provider,
) {
	*provider.script.delegate.lock().unwrap() = false;
	let mut world = world(provider).await;
	let coordinator = world.run_coordinator(WITHOUT_DELEGATION).await;
	add_policies(
		&world,
		&["bob", "carol"],
		vec![bob_cannot_discover(), carol_reads_nothing()],
	)
	.await;
	let bob = subject_credential(&world.f.store, "acme", "bob").await;
	let carol = subject_credential(&world.f.store, "acme", "carol").await;
	let path = format!("/api/runs/{}/inference/stream", coordinator.id);
	let original = limit_pool(&mut world, 1).await;
	let response = world
		.app
		.clone()
		.native_oneshot(
			http::Request::get(path.clone())
				.header("authorization", format!("Bearer {}", world.token))
				.body(bytes::Bytes::new())
				.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(response.status, 200);
	drop(response);
	for credential in [&bob, &carol] {
		let (status, body) = common::get(&world.app, credential, &path).await;
		assert_eq!(status, 403, "{body}");
		assert_ne!(body["code"], "node_unreachable", "{body}");
	}
	restore_pool(&mut world, original).await;
	world.close().await;
}

/// Entries of `unconfirmed` for one kind, as `(node, workspace, count)`.
fn unconfirmed_of(body: &Value, kind: &str) -> Vec<(String, String, u64)> {
	body["unconfirmed"]
		.as_array()
		.unwrap_or_else(|| panic!("`unconfirmed` is always present: {body}"))
		.iter()
		.filter(|entry| entry["kind"] == kind)
		.map(|entry| {
			(
				entry["node"].as_str().unwrap().to_owned(),
				entry["workspace_id"].as_str().unwrap().to_owned(),
				entry["count"].as_u64().unwrap(),
			)
		})
		.collect()
}

/// Bob may read the Workspace but may not discover Nodes (#770, decision 4).
async fn credential_without_discovery(world: &World) -> String {
	let (status, current) = request(
		&world.app,
		&world.f.config.api_token,
		"GET",
		"/api/authorization/acme",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{current}");
	let mut bundle = current["bundle"].clone();
	bundle["subjects"]["bob"] = json!({"kind":"user"});
	bundle["policies"].as_array_mut().unwrap().push(json!({
		"id":"bob-cannot-discover","effect":"deny","subjects":{"ids":["bob"]},
		"actions":["federation.discover"],"resources":{"kinds":["node"]}
	}));
	let (status, body) = request(
		&world.app,
		&world.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":current["revision"],"bundle":bundle}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	subject_credential(&world.f.store, "acme", "bob").await
}

/// Produce and read a real remote message under an explicitly disclosed Home context.
async fn establish_delegated_read(world: &World, created: usize) -> Task {
	world
		.f
		.store
		.keyless
		.declare_hosted_origin_of(&world.model_server.url)
		.unwrap();
	world.run_coordinator(3).await;
	// Prepare the explicitly disclosed Home context before the tool delegates;
	// the ordinary delegation route reuses a matching prepared grant.
	let delegated = world.handed_over().await;
	let (status, prepared) = request(&world.app, &world.token, "POST", &format!("/api/tasks/{}/remote-grants", delegated.id), json!({"id":Uuid::new_v4(),"node_id":world.sibling,"agent":{"id":"sibling-agent","version":"1.0.0"},"ttl_seconds":3600,"semantic":{"mode":"required_home","embedding":{"id":"home-embedding","version":"1.0.0"}}})).await;
	assert_eq!(status, 200, "{prepared}");
	world.advance_coordinator(4).await;
	let receiver = world.receiver().await;
	assert_eq!(
		receiver.phase().as_str(),
		"COMPLETED",
		"{:?}",
		receiver.error
	);
	let revisions: (i64, i64) = sqlx::query_as("SELECT t.revision, e.task_revision FROM tasks t JOIN authorization_remote_execution e ON e.task_id=t.id WHERE t.id=$1")
        .bind(delegated.id).fetch_one(world.f.store.pool.fixture_driver()).await.unwrap();
	assert_eq!(
		revisions.0, revisions.1,
		"the completed Task stays bound to its grant: {revisions:?}"
	);
	let remote_outputs: Vec<(String, Uuid)> = sqlx::query_as(
		"SELECT resource_kind, resource_id FROM authorization_remote_outputs WHERE workspace_id = $1",
	)
	.bind(receiver.workspace_id)
	.fetch_all(world.f.store.pool.fixture_driver())
	.await
	.unwrap();
	let message = remote_outputs
		.iter()
		.find_map(|(kind, id)| (kind == "message").then_some(*id))
		.expect("the receiver recorded a remote message output");
	*world.script.result_message.lock().unwrap() = Some(message);
	world
		.script
		.child_completed
		.store(true, std::sync::atomic::Ordering::SeqCst);
	let coordinator = world.advance_coordinator(created + 6).await;
	let (status, details) = world.get(&format!("/api/runs/{}", coordinator.id)).await;
	assert_eq!(status, 200, "{details}");
	let read = details["invocations"]
		.as_array()
		.unwrap()
		.iter()
		.find(|invocation| invocation["tool"] == "workspace_read")
		.expect("the coordinator read the delegated message");
	assert_eq!(read["status"], "COMPLETED", "{read}");
	assert!(read["result"].get("error").is_none(), "{read}");
	let observed: Value =
		serde_json::from_str(read["result"]["content"].as_str().unwrap()).unwrap();
	assert_eq!(observed["content"], "Done", "{read}");
	assert_eq!(observed["id"], message.to_string(), "{read}");
	let reads: Vec<(String, Uuid)> = sqlx::query_as(
		"SELECT resource_kind, resource_id FROM authorization_run_reads WHERE run_id = $1",
	)
	.bind(coordinator.id)
	.fetch_all(world.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert!(
		reads
			.iter()
			.any(|(kind, id)| kind == "message" && *id == message),
		"read={read}, reads={reads:?}"
	);
	delegated
}

/// A Run that delegated to the second Node and then created Tasks follows the
/// delegated Run's results. Those Tasks and the RequiredHome delegated Task
/// need the Node's answer, so while it is silent they are each counted once
/// and other rows are not (#770, decisions 1-3). A reader who may not discover the Node
/// sees neither an entry nor those rows, and a single read still asks while a
/// second list does not wait. The Tasks return once the Node answers.
#[rstest::rstest]
#[tokio::test]
async fn a_list_counts_the_tasks_that_follow_a_delegated_run_under_the_silent_node_and_a_reader_without_discovery_sees_no_entry(
	#[future(awt)] provider: Provider,
) {
	// The Agent delegates its first Task, then creates three more.
	provider
		.script
		.tasks
		.store(4, std::sync::atomic::Ordering::SeqCst);
	provider
		.script
		.read_delegated
		.store(true, std::sync::atomic::Ordering::SeqCst);
	let mut world = world(provider).await;
	let delegated = establish_delegated_read(&world, 4).await;
	let children = world.all_handed_over().await;
	assert_eq!(children.len(), 4);
	let workspace = children[0].workspace_id.to_string();
	let source =
		common::register_source(&world.f, &world.app, "aidash://graph-asker", "acme").await;
	let credential = common::issued_credential(&world.f.store, "acme", "alice").await;
	let (status, mapping) = common::set_peer_mapping(
		&world.f,
		"acme",
		"alice",
		json!({
			"node": source.receiver.node, "source_node": source.address(), "source_subject": "viewer",
			"credential_id": credential["credential"]["id"], "enabled": true, "expected_revision": 0,
		}),
	)
	.await;
	assert_eq!(status, 200, "{mapping}");
	let bob = credential_without_discovery(&world).await;
	// Arrange: every Node answers, so nothing is left out.
	let (status, tasks) = world.get("/api/tasks").await;
	assert_eq!(status, 200, "{tasks}");
	assert_eq!(tasks["unconfirmed"], json!([]), "{tasks}");
	assert!(children.iter().all(|child| listed(&tasks, child.id)));

	// Act: the second Node stops answering.
	let original = limit_pool(&mut world, 1).await;
	let first = std::time::Instant::now();
	let (status, tasks) = world.get("/api/tasks").await;
	let first = first.elapsed();
	// Assert: the Tasks that need the Node are left out and counted under it,
	// the rest stay, and every missing row is counted.
	assert_eq!(status, 200, "{tasks}");
	let hidden: Vec<_> = children
		.iter()
		.filter(|child| !listed(&tasks, child.id))
		.collect();
	assert_eq!(
		hidden.len(),
		4,
		"the delegated Task and three following Tasks are left out: {tasks}"
	);
	assert!(
		hidden.iter().any(|child| child.id == delegated.id),
		"{tasks}"
	);
	assert_eq!(
		hidden
			.iter()
			.filter(|child| child.id != delegated.id)
			.count(),
		3,
		"{tasks}"
	);
	assert!(listed(&tasks, world.task), "{tasks}");
	let expected = vec![(
		world.sibling.to_string(),
		workspace.clone(),
		hidden.len() as u64,
	)];
	assert_eq!(unconfirmed_of(&tasks, "task"), expected, "{tasks}");
	let (status, graph) = common::post_graph(
		&world.app,
		&source,
		"viewer",
		&json!({
			"subject":"viewer", "scope_workspace":null, "mode":"mesh", "kinds":["task"],
			"relations":[], "hours":0, "limit":80, "cursor":null
		}),
	)
	.await;
	assert_eq!(status, 200, "{graph}");
	assert_eq!(
		unconfirmed_of(&graph, "task"),
		vec![(source.receiver.to_string(), workspace.clone(), 4)],
		"{graph}"
	);
	assert!(
		!graph.to_string().contains(&world.sibling.to_string()),
		"{graph}"
	);
	let single = format!("/api/tasks/{}/runs", hidden[0].id);
	// The same Node is named in the state and the Workspace.
	let (status, state) = world.get("/api/state").await;
	assert_eq!(status, 200, "{state}");
	assert_eq!(unconfirmed_of(&state, "task"), expected, "{state}");
	let (status, snapshot) = world.get(&format!("/api/workspaces/{workspace}")).await;
	assert_eq!(status, 200, "{snapshot}");
	assert_eq!(unconfirmed_of(&snapshot, "task"), expected, "{snapshot}");
	// The events of the missing rows are counted under the same Node.
	let (status, events) = world
		.get(&format!("/api/events?workspace_id={workspace}"))
		.await;
	assert_eq!(status, 200, "{events}");
	let counted: u64 = unconfirmed_of(&events, "event")
		.iter()
		.map(|(node, ws, count)| {
			assert_eq!(
				(node, ws),
				(&world.sibling.to_string(), &workspace),
				"{events}"
			);
			*count
		})
		.sum();
	assert!(
		counted > 0,
		"the events of the missing Tasks are counted: {events}"
	);
	// A reader who may not discover the Node learns nothing about it.
	let (status, tasks_bob) = request(&world.app, &bob, "GET", "/api/tasks", Value::Null).await;
	assert_eq!(status, 200, "{tasks_bob}");
	assert_eq!(tasks_bob["unconfirmed"], json!([]), "{tasks_bob}");
	assert!(hidden.iter().all(|child| !listed(&tasks_bob, child.id)));
	// A second list within the memory does not wait on the Node again, and a
	// single read still asks it.
	let second = std::time::Instant::now();
	let (status, tasks) = world.get("/api/tasks").await;
	let second = second.elapsed();
	assert_eq!(status, 200, "{tasks}");
	assert_eq!(unconfirmed_of(&tasks, "task"), expected, "{tasks}");
	assert!(
		second < first || second < Duration::from_millis(500),
		"the second list waited {second:?} after {first:?}"
	);
	let asking = std::time::Instant::now();
	let (status, body) = world.get(&single).await;
	assert_eq!(status, 503, "{body}");
	assert_eq!(body["code"], "node_unreachable", "{body}");
	assert!(
		asking.elapsed() >= Duration::from_millis(900),
		"a single read asks the Node: {:?}",
		asking.elapsed()
	);

	// The Node answers again. A single read asks and clears what the lists
	// remembered, so the Tasks return and the count goes.
	world.f.store.pool = original;
	world.app = common::application_with_settings(
		world.f.clone(),
		aidash_server::http::Settings {
			auth_burst: 10000,
			..Default::default()
		},
	)
	.await;
	let (status, body) = world.get(&single).await;
	assert_eq!(status, 200, "{body}");
	let (status, tasks) = world.get("/api/tasks").await;
	assert_eq!(status, 200, "{tasks}");
	assert_eq!(tasks["unconfirmed"], json!([]), "{tasks}");
	assert!(children.iter().all(|child| listed(&tasks, child.id)));
	world.close().await;
}

/// A reader who may discover the Node but is refused `run.read` sees no Run
/// entry and no Human Request entry for the Runs a silent Node holds back: a
/// row counts only if every other check for this reader passed (#770,
/// decisions 3 and 4).
#[rstest::rstest]
#[tokio::test]
async fn a_reader_refused_run_read_is_told_of_no_unconfirmed_run(
	#[future(awt)] provider: Provider,
) {
	provider
		.script
		.tasks
		.store(3, std::sync::atomic::Ordering::SeqCst);
	provider
		.script
		.read_delegated
		.store(true, std::sync::atomic::Ordering::SeqCst);
	let mut world = world(provider).await;
	establish_delegated_read(&world, 3).await;
	// Carol may read and discover, but not read Runs.
	let (status, current) = request(
		&world.app,
		&world.f.config.api_token,
		"GET",
		"/api/authorization/acme",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{current}");
	let mut bundle = current["bundle"].clone();
	bundle["subjects"]["carol"] = json!({"kind":"user"});
	bundle["policies"].as_array_mut().unwrap().push(json!({
		"id":"carol-reads-no-runs","effect":"deny","subjects":{"ids":["carol"]},
		"actions":["run.read"],"resources":{"kinds":["*"]}
	}));
	let (status, body) = request(
		&world.app,
		&world.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":current["revision"],"bundle":bundle}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let carol = subject_credential(&world.f.store, "acme", "carol").await;
	let _original = limit_pool(&mut world, 1).await;
	let (status, alice) = world.get("/api/state").await;
	assert_eq!(status, 200, "{alice}");
	assert!(
		!unconfirmed_of(&alice, "run").is_empty(),
		"the person who may read Runs is told of the Run the Node holds back: {alice}"
	);
	let (status, carol_state) = request(&world.app, &carol, "GET", "/api/state", Value::Null).await;
	assert_eq!(status, 200, "{carol_state}");
	assert!(
		unconfirmed_of(&carol_state, "run").is_empty(),
		"{carol_state}"
	);
	assert!(
		unconfirmed_of(&carol_state, "human_request").is_empty(),
		"{carol_state}"
	);
	world.close().await;
}

/// A Node that answers and refuses is a refusal: no Task is counted as
/// unconfirmed for it (#770, decision 3).
#[rstest::rstest]
#[tokio::test]
async fn a_task_the_other_node_refuses_is_not_counted(#[future(awt)] provider: Provider) {
	let world = world(provider).await;
	world.run_coordinator(WITH_DELEGATION).await;
	let authorization = aidash_server::authorization::Authorization {
		pool: world.f.store.pool.clone(),
	};
	let reference = aidash_domain::registry::EntityRef {
		id: "sibling-agent".into(),
		version: "1.0.0".into(),
	};
	let binding = authorization
		.catalog("acme", &world.sibling.node)
		.await
		.unwrap()
		.into_iter()
		.find(|binding| binding.entry_id == reference.id)
		.expect("the approval exists");
	authorization
		.set_catalog(
			"acme",
			&world.sibling.node,
			&reference,
			binding.revision,
			false,
			"deployment_administrator",
		)
		.await
		.unwrap();
	let (status, tasks) = world.get("/api/tasks").await;
	assert_eq!(status, 200, "{tasks}");
	assert_eq!(tasks["unconfirmed"], json!([]), "{tasks}");
	world.close().await;
}

/// Serves the application from `pool` again, once the Node answers.
async fn restore_pool(world: &mut World, pool: aidash_server::database::native::Pool) {
	world.f.store.pool = pool;
	world.app = common::application_with_settings(
		world.f.clone(),
		aidash_server::http::Settings {
			auth_burst: 10000,
			..Default::default()
		},
	)
	.await;
}

/// A generation request for the Task the coordinator created, approved and
/// queued: its requester's view of that Task depends on a Run that read on
/// the second Node.
async fn queued_generation(world: &World) -> Uuid {
	let child = world.handed_over().await;
	let (_, mut template) =
		common::get(&world.app, &world.token, "/api/registry/research/1.0.0").await;
	template["id"] = json!("research-template");
	template["capabilities"] = json!(["special.research"]);
	let spec = json!({"enabled":true,"template":template,"permissions":{"roles":[],"groups":[],"attributes":{"team":"research"}},"approval_required":true,"limits":{"max_agents":4,"max_concurrent":2,"max_depth":2,"token_budget":800000,"tokens_per_agent":200000,"lifetime_seconds":3600}});
	let (status, body) = common::post(
		&world.app,
		&world.token,
		"/api/generation/acme/policies/research",
		json!({"expected_revision":0,"spec":spec}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, assigned) = common::post(&world.app, &world.token, &format!("/api/generation/acme/tasks/{}/assign", child.id), json!({"policy_id":"research","reason":"specialist for a Task another Node's Agent was found for"}))
	.await;
	assert_eq!(status, 200, "{assigned}");
	let id: Uuid = assigned["generation"]["id"]
		.as_str()
		.unwrap()
		.parse()
		.unwrap();
	let (status, approved) = common::post(
		&world.app,
		&world.token,
		&format!("/api/generation/acme/requests/{id}/control"),
		json!({"action":"approve","reason":"reviewed definition"}),
	)
	.await;
	assert_eq!(status, 200, "{approved}");
	assert_eq!(approved["status"], "QUEUED");
	id
}

async fn generation_status(world: &World, id: Uuid) -> String {
	sqlx::query_scalar("SELECT status FROM generation_requests WHERE id = $1")
		.bind(id)
		.fetch_one(world.f.store.pool.fixture_driver())
		.await
		.unwrap()
}

/// A queued generation request is activated under the exclusive lock. Its Task's
/// Read dependency on the second Node is decided here (#769), so it activates while
/// that Node is silent; afterwards a refusal to stop it is final and names no Node.
#[rstest::rstest]
#[tokio::test]
async fn a_queued_generation_request_activates_while_a_node_is_silent(
	#[future(awt)] provider: Provider,
) {
	*provider.script.delegate.lock().unwrap() = false;
	*provider.script.requirements.lock().unwrap() = json!({"capability":"special.research"});
	let mut world = world(provider).await;
	world.run_coordinator(WITHOUT_DELEGATION).await;
	let job = queued_generation(&world).await;
	let control = format!("/api/generation/acme/requests/{job}/control");
	add_policies(&world, &["carol"], vec![carol_reads_nothing()]).await;
	let carol = subject_credential(&world.f.store, "acme", "carol").await;

	let original = limit_pool(&mut world, 1).await;
	aidash_server::generation::provision::reconcile(&world.f)
		.await
		.expect("a silent Node does not abandon the tick");
	assert_eq!(generation_status(&world, job).await, "ACTIVE");
	restore_pool(&mut world, original).await;

	let stop = json!({"action":"stop","reason":"changed my mind"});
	let (status, body) = common::post(&world.app, &carol, &control, stop.clone()).await;
	assert_eq!(status, 403, "{body}");
	assert_ne!(body["code"], "node_unreachable", "{body}");
	let (status, body) = common::post(&world.app, &world.token, &control, stop).await;
	assert_eq!(status, 200, "{body}");
	world.close().await;
}

/// A refusal at activation still writes `FAILED`: a requester who may no
/// longer read the Task is the admission denial it always was.
#[rstest::rstest]
#[tokio::test]
async fn a_real_refusal_at_activation_still_fails_the_request(#[future(awt)] provider: Provider) {
	*provider.script.delegate.lock().unwrap() = false;
	*provider.script.requirements.lock().unwrap() = json!({"capability":"special.research"});
	let world = world(provider).await;
	world.run_coordinator(WITHOUT_DELEGATION).await;
	let job = queued_generation(&world).await;
	let (status, current) = common::get(
		&world.app,
		&world.f.config.api_token,
		"/api/authorization/acme",
	)
	.await;
	assert_eq!(status, 200, "{current}");
	let mut bundle = current["bundle"].clone();
	bundle["policies"].as_array_mut().unwrap().push(json!({
		"id":"alice-reads-no-workspace","effect":"deny","subjects":{"ids":["alice"]},
		"actions":["workspace.read"],"resources":{"kinds":["*"]}
	}));
	let (status, body) = common::post(
		&world.app,
		&world.f.config.api_token,
		"/api/authorization/acme",
		json!({"expected_revision":current["revision"],"bundle":bundle}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	aidash_server::generation::provision::reconcile(&world.f)
		.await
		.unwrap();
	assert_eq!(generation_status(&world, job).await, "FAILED");
	world.close().await;
}

/// #751 bounded a check of the second Node at 10 s so that a read did not wait for
/// the pool's 60 s (items 10 and 12). Since #769 that read asks no Node, so with a
/// pool that would make a peer wait 60 s it answers at once.
#[rstest::rstest]
#[tokio::test]
async fn a_read_decided_here_does_not_wait_for_a_silent_node(#[future(awt)] provider: Provider) {
	*provider.script.delegate.lock().unwrap() = false;
	let mut world = world(provider).await;
	world.run_coordinator(WITHOUT_DELEGATION).await;
	let child = world.handed_over().await;
	let path = format!("/api/tasks/{}/runs", child.id);
	let original = limit_pool_waiting(&mut world, 1, Duration::from_secs(60)).await;
	let started = std::time::Instant::now();
	let (status, body) = world.get(&path).await;
	let elapsed = started.elapsed();
	assert_eq!(status, 200, "{body}");
	assert!(
		elapsed < Duration::from_secs(9),
		"the read waited for a Node: {elapsed:?}"
	);
	restore_pool(&mut world, original).await;
	world.close().await;
}

/// A Run that was delegated and whose Node is then deleted. What the Run that
/// delegated produced is decided here (#769), so the Task it created and its own
/// view stay readable. The Run on the other Node stays refused at Home, and no
/// Node is blamed.
#[rstest::rstest]
#[tokio::test]
async fn deleting_the_delegated_node_hides_nothing_the_home_decides(
	#[future(awt)] provider: Provider,
) {
	let world = world(provider).await;
	let coordinator = world.run_coordinator(WITH_DELEGATION).await;
	let receiver = world.receiver().await;
	stops_at_data_protection(&receiver);
	let child = world.handed_over().await;
	let paths = [
		format!("/api/tasks/{}/runs", child.id),
		format!("/api/runs/{}", coordinator.id),
	];
	let (status, body) = administer_node(
		&world.f,
		"acme",
		&format!("/api/tenants/acme/nodes/{}/deletion", world.sibling.node),
		json!({"confirmation":world.sibling.node}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	for path in &paths {
		let (status, body) = world.get(path).await;
		assert_eq!(status, 200, "{path}: {body}");
	}
	let (status, body) = world.get(&format!("/api/runs/{}", receiver.id)).await;
	assert_eq!(status, 403, "{body}");
	assert_ne!(body["code"], "node_unreachable", "{body}");
	world.close().await;
}

/// Resuming a paused Run checks what it read again. Its read on the second Node is
/// decided here (#769), so a silent Node does not keep it paused.
#[rstest::rstest]
#[tokio::test]
async fn resuming_while_a_node_is_silent_resumes(#[future(awt)] provider: Provider) {
	*provider.script.delegate.lock().unwrap() = false;
	let mut world = world(provider).await;
	let run = world.run_coordinator(WITHOUT_DELEGATION).await;
	let control = format!("/api/runs/{}/control", run.id);
	let (status, body) = world_post(&world, &control, json!({"action":"pause"})).await;
	assert_eq!(status, 200, "{body}");
	let original = limit_pool(&mut world, 1).await;
	let (status, body) = world_post(&world, &control, json!({"action":"resume"})).await;
	assert_eq!(status, 200, "{body}");
	restore_pool(&mut world, original).await;
	assert_eq!(world.coordinator().await.control.as_str(), "ACTIVE");
	world.close().await;
}

async fn world_post(world: &World, path: &str, body: Value) -> (u16, Value) {
	common::post(&world.app, &world.token, path, body).await
}

/// #751 made the Node a reader asked run inside the reader's shared hold of the
/// distribution lock without requesting it again (decision 13). Since #769 the
/// read takes no catalog admission, so a writer holding that lock does not hold
/// the read up.
#[rstest::rstest]
#[tokio::test]
async fn a_read_does_not_wait_for_a_writer_holding_the_distribution_lock(
	#[future(awt)] provider: Provider,
) {
	*provider.script.delegate.lock().unwrap() = false;
	let world = world(provider).await;
	world.run_coordinator(WITHOUT_DELEGATION).await;
	let child = world.handed_over().await;
	let path = format!("/api/tasks/{}/runs", child.id);
	let driver = world.f.store.pool.fixture_driver().clone();
	// The key is the one `marketplace::services::storage::lock` takes.
	let mut writer = driver.begin().await.unwrap();
	sqlx::query("SELECT pg_advisory_xact_lock(74003201)")
		.execute(&mut *writer)
		.await
		.unwrap();
	let started = std::time::Instant::now();
	let (status, body) = tokio::time::timeout(Duration::from_secs(30), world.get(&path))
		.await
		.expect("the read does not wait for the writer");
	let elapsed = started.elapsed();
	writer.commit().await.unwrap();
	assert_eq!(status, 200, "{body}");
	assert!(
		elapsed < Duration::from_secs(8),
		"the read waited for the writer: {elapsed:?}"
	);
	world.close().await;
}

/// A deny on `registry.read` for `sibling-agent`, written as for any definition of the Tenant.
fn alice_may_not_read_sibling_agent() -> Value {
	json!({"id":"no-definition","effect":"deny","subjects":{"ids":["alice"]},
		"actions":["registry.read"],"resources":{"kinds":["agent"],"ids":["sibling-agent"]}})
}

/// A deny on `registry.read` for Agents found on the Node `node`.
fn alice_may_not_read_agents_found_on(id: &str, node: &str) -> Value {
	json!({"id":id,"effect":"deny","subjects":{"ids":["alice"]},
		"actions":["registry.read"],"resources":{"kinds":["agent"]},
		"condition":{"op":"eq","left":{"source":"resource","path":"/remote_node"},
			"right":{"source":"literal","value":node}}})
}

/// An Agent found on another Node of the Tenant is the Tenant's own definition,
/// so a deny on `registry.read` that names it hides the Run as for a local one
/// (#856, decisions 1 and 3).
#[rstest::rstest]
#[tokio::test]
async fn a_denied_registry_read_hides_a_run_that_found_the_agent_on_another_node(
	#[future(awt)] provider: Provider,
) {
	*provider.script.delegate.lock().unwrap() = false;
	let world = world(provider).await;
	let coordinator = world.run_coordinator(WITHOUT_DELEGATION).await;
	let run = format!("/api/runs/{}", coordinator.id);
	let (status, body) = world.get(&run).await;
	assert_eq!(status, 200, "{body}");

	add_policies(&world, &[], vec![alice_may_not_read_sibling_agent()]).await;
	let (status, body) = world.get(&run).await;
	assert_eq!(status, 403, "{body}");
	assert_eq!(body["run_reads"], json!({"kind":"definition_not_readable"}));
	assert!(!body.to_string().contains("sibling-agent"), "{body}");
	world.close().await;
}

/// `remote_node` still singles out the Node an Agent was found on (#856, decision 2).
#[rstest::rstest]
#[tokio::test]
async fn a_remote_node_condition_hides_only_what_was_found_on_that_node(
	#[future(awt)] provider: Provider,
) {
	*provider.script.delegate.lock().unwrap() = false;
	let world = world(provider).await;
	let coordinator = world.run_coordinator(WITHOUT_DELEGATION).await;
	let run = format!("/api/runs/{}", coordinator.id);

	let elsewhere = world
		.f
		.config
		.deployment
		.node(NodeName::parse("quiet-heron-x2b7k9qa").unwrap())
		.to_string();
	add_policies(
		&world,
		&[],
		vec![alice_may_not_read_agents_found_on("elsewhere", &elsewhere)],
	)
	.await;
	let (status, body) = world.get(&run).await;
	assert_eq!(status, 200, "{body}");

	add_policies(
		&world,
		&[],
		vec![alice_may_not_read_agents_found_on(
			"sibling",
			&world.sibling.to_string(),
		)],
	)
	.await;
	let (status, body) = world.get(&run).await;
	assert_eq!(status, 403, "{body}");
	assert_eq!(body["run_reads"], json!({"kind":"definition_not_readable"}));
	world.close().await;
}

/// Discovery names the Agent the same way, so the deny keeps it out of what a
/// person discovers (#856, decision 4).
#[rstest::rstest]
#[tokio::test]
async fn a_denied_registry_read_leaves_the_agent_out_of_discovery(
	#[future(awt)] provider: Provider,
) {
	let world = world(provider).await;
	let discover = || async {
		let (status, body) = request(
			&world.app,
			&world.token,
			"POST",
			"/api/discover",
			json!({"capability":"sibling.discovery"}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		body["agents"]
			.as_array()
			.unwrap_or_else(|| panic!("a list of Agents: {body}"))
			.iter()
			.any(|agent| agent["entity"]["id"] == "sibling-agent")
	};
	assert!(discover().await, "the second Node offers sibling-agent");
	add_policies(&world, &[], vec![alice_may_not_read_sibling_agent()]).await;
	assert!(!discover().await, "the deny keeps sibling-agent out");
	world.close().await;
}

/// A running Run that found the Agent stops at its next worker boundary once
/// its owner may no longer read it (#856).
#[rstest::rstest]
#[tokio::test]
async fn a_denied_registry_read_stops_a_running_run_that_found_the_agent(
	#[future(awt)] provider: Provider,
) {
	*provider.script.delegate.lock().unwrap() = false;
	let world = world(provider).await;
	// The discovery is saved once the next request is made.
	world.run_coordinator(2).await;
	add_policies(&world, &[], vec![alice_may_not_read_sibling_agent()]).await;
	let worker = Harness {
		federation: world.f.clone(),
	};
	let stopped = tokio::time::timeout(Duration::from_secs(120), async {
		loop {
			worker.worker_once().await.unwrap();
			let run = world.coordinator().await;
			assert!(
				world.script.stage.load(std::sync::atomic::Ordering::SeqCst) < WITHOUT_DELEGATION,
				"the coordinator went on: {:?}",
				run.phase()
			);
			if run.error.is_some() || matches!(run.phase().as_str(), "FAILED" | "CANCELLED") {
				return run;
			}
			tokio::time::sleep(Duration::from_millis(20)).await;
		}
	})
	.await
	.expect("the coordinator stops");
	assert_ne!(stopped.phase().as_str(), "COMPLETED", "{:?}", stopped.error);
	world.close().await;
}
