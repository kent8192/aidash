#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
//! Reads that depend on another Node of the same Tenant and Deployment (#751).
//!
//! A Run on the default Node discovers an Agent that only the second Node
//! offers, creates a Task, and hands it over. Reading that Task then needs the
//! second Node's answer, which an authority built from a `Store` could not ask
//! before the Store held the server's in-process handlers.
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
	/// How many requests the coordinator has made.
	stage: std::sync::atomic::AtomicUsize,
}

#[rstest::fixture]
fn script() -> Arc<Script> {
	Arc::new(Script {
		sibling: Mutex::new(None),
		delegate: Mutex::new(true),
		stage: Default::default(),
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
	if !transcript.contains(COORDINATOR_TASK) {
		// The receiver on the second Node.
		return json!({"role":"assistant","content":"Done"});
	}
	match script
		.stage
		.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
	{
		0 => tool_call(
			"discover",
			"agent_discover",
			json!({"capability":"sibling.discovery"}),
		),
		1 => tool_call(
			"create",
			"task_create",
			json!({"title":"Hand over","description":HANDED_OVER,"requirements":{}}),
		),
		2 if *script.delegate.lock().unwrap() => {
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
	_provider: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	_runtime: RuntimeFixture,
	url: String,
	schema: String,
}

async fn world(provider: Provider) -> World {
	let (f, url, schema) = provider.application.runtime.parts();
	let runtime = provider.application.runtime.clone();
	let app = provider.application.application;
	let endpoint = provider.server.url.clone();
	let (_policy, token, task) = bootstrap(&f, &app, &endpoint).await;
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
		_provider: provider.server,
		_runtime: runtime,
		url,
		schema,
	}
}

impl World {
	async fn get(&self, path: &str) -> (u16, Value) {
		request(&self.app, &self.token, "GET", path, Value::Null).await
	}

	/// Alice claims her Task with `research`, whose Run follows the script. The
	/// worker steps until the coordinator has made `requests` model requests,
	/// which is when what it did before has been saved. A Run with an open
	/// child Task does not complete, so completion is not awaited.
	async fn run_coordinator(&self, requests: usize) -> Run {
		let (status, claimed) = request(
			&self.app,
			&self.token,
			"POST",
			&format!("/api/tasks/{}/claim", self.task),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
		)
		.await;
		assert_eq!(status, 200, "{claimed}");
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

/// Withdrawing the Agent's approval on the other Node is a refusal that node
/// can answer, so the read stays 403 and is not reported as unreachable.
#[rstest::rstest]
#[tokio::test]
async fn a_withdrawn_approval_on_the_other_node_still_refuses_the_read(
	#[future(awt)] provider: Provider,
) {
	*provider.script.delegate.lock().unwrap() = false;
	let world = world(provider).await;
	world.run_coordinator(WITHOUT_DELEGATION).await;
	let child = world.handed_over().await;
	let (status, body) = world.get(&format!("/api/tasks/{}/runs", child.id)).await;
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
	let (status, body) = world.get(&format!("/api/tasks/{}/runs", child.id)).await;
	assert_eq!(status, 403, "{body}");
	assert_ne!(body["code"], "node_unreachable", "{body}");
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
	let (status, body) = request(
		&world.app,
		&world.token,
		"POST",
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
	let small = world
		.f
		.store
		.pool
		.options()
		.clone()
		.max_connections(connections)
		.min_connections(0)
		.acquire_timeout(Duration::from_secs(1))
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

/// A Node that does not answer leaves the read unconfirmed: 503 naming the
/// Node for a single read, the item left out of a list, 403 for a reader who
/// may not discover the Node, and nothing remembered once the Node answers.
#[rstest::rstest]
#[tokio::test]
async fn a_node_that_does_not_answer_leaves_the_read_unconfirmed(
	#[future(awt)] provider: Provider,
) {
	*provider.script.delegate.lock().unwrap() = false;
	let mut world = world(provider).await;
	world.run_coordinator(WITHOUT_DELEGATION).await;
	let child = world.handed_over().await;
	// Bob may read the Workspace but may not discover the second Node.
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
	let bob = subject_credential(&world.f.store, "acme", "bob").await;
	let single = format!("/api/tasks/{}/runs", child.id);
	let (status, tasks) = world.get("/api/tasks").await;
	assert_eq!(status, 200, "{tasks}");
	assert!(listed(&tasks, child.id), "{tasks}");

	let original = limit_pool(&mut world, 1).await;
	// A single read names the Node and discloses nothing else.
	let (status, body) = world.get(&single).await;
	assert_eq!(status, 503, "{body}");
	assert_eq!(body["code"], "node_unreachable", "{body}");
	assert_eq!(body["node"], json!(world.sibling.to_string()), "{body}");
	assert!(
		body["error"]
			.as_str()
			.unwrap()
			.contains(&world.sibling.to_string())
	);
	assert!(body.get("title").is_none(), "{body}");
	// A list leaves the item out and still answers.
	let (status, tasks) = world.get("/api/tasks").await;
	assert_eq!(status, 200, "{tasks}");
	assert!(
		!listed(&tasks, child.id),
		"an unconfirmed item is left out: {tasks}"
	);
	// A reader who may not discover the Node learns nothing about it.
	let (status, body) = request(&world.app, &bob, "GET", &single, Value::Null).await;
	assert_eq!(status, 403, "{body}");
	assert_ne!(body["code"], "node_unreachable", "{body}");
	world.f.store.pool = original;
	world.app = common::application_with_settings(
		world.f.clone(),
		aidash_server::http::Settings {
			auth_burst: 10000,
			..Default::default()
		},
	)
	.await;

	// Once the Node answers again nothing was remembered against the reader.
	let (status, body) = world.get(&single).await;
	assert_eq!(status, 200, "{body}");
	let (status, tasks) = world.get("/api/tasks").await;
	assert_eq!(status, 200, "{tasks}");
	assert!(listed(&tasks, child.id), "{tasks}");
	world.close().await;
}

/// With a pool too small for the nested checks, every read still ends within
/// the limit, none hangs, and the pool serves the next read (#751, item 12).
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
/// is deleted (#751, item 18). No route reaches a Node that is not live, so the
/// read is refused, not reported as unreachable. This pins the behaviour.
#[rstest::rstest]
#[tokio::test]
async fn deleting_the_discovered_node_keeps_the_outputs_hidden(#[future(awt)] provider: Provider) {
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
	assert_eq!(status, 403, "{body}");
	assert_ne!(body["code"], "node_unreachable", "{body}");
	world.close().await;
}

/// Opening a Run's live inference stream is a person's read of one thing. A
/// Node that does not answer makes it `503 node_unreachable`, which the Web
/// retries (`permanentlyDenied` stops only on 403 and 404); a real refusal and
/// a reader who may not discover the Node stay `403` (#751).
#[rstest::rstest]
#[tokio::test]
async fn opening_a_run_stream_while_a_node_is_silent_is_retryable_and_a_refusal_is_not(
	#[future(awt)] provider: Provider,
) {
	*provider.script.delegate.lock().unwrap() = false;
	let mut world = world(provider).await;
	let coordinator = world.run_coordinator(WITHOUT_DELEGATION).await;
	// Bob may not discover the second Node; Carol may not read Workspaces.
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
	bundle["subjects"]["carol"] = json!({"kind":"user"});
	bundle["policies"].as_array_mut().unwrap().extend([
		json!({"id":"bob-cannot-discover","effect":"deny","subjects":{"ids":["bob"]},
			"actions":["federation.discover"],"resources":{"kinds":["node"]}}),
		json!({"id":"carol-reads-nothing","effect":"deny","subjects":{"ids":["carol"]},
			"actions":["workspace.read"],"resources":{"kinds":["*"]}}),
	]);
	let (status, body) = request(
		&world.app,
		&world.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":current["revision"],"bundle":bundle}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let bob = subject_credential(&world.f.store, "acme", "bob").await;
	let carol = subject_credential(&world.f.store, "acme", "carol").await;
	let path = format!("/api/runs/{}/inference/stream", coordinator.id);
	let original = limit_pool(&mut world, 1).await;
	// The Node is silent: the person is told which one, and may retry.
	let (status, body) = world.get(&path).await;
	assert_eq!(status, 503, "{body}");
	assert_eq!(body["code"], "node_unreachable", "{body}");
	assert_eq!(body["node"], json!(world.sibling.to_string()), "{body}");
	// A reader who may not discover that Node is refused, naming nothing.
	let (status, body) = request(&world.app, &bob, "GET", &path, Value::Null).await;
	assert_eq!(status, 403, "{body}");
	assert_ne!(body["code"], "node_unreachable", "{body}");
	// A refusal that has nothing to do with the Node is still final.
	let (status, body) = request(&world.app, &carol, "GET", &path, Value::Null).await;
	assert_eq!(status, 403, "{body}");
	assert_ne!(body["code"], "node_unreachable", "{body}");
	world.f.store.pool = original;
	world.app = common::application_with_settings(
		world.f.clone(),
		aidash_server::http::Settings {
			auth_burst: 10000,
			..Default::default()
		},
	)
	.await;
	// The Node answers again: nothing was remembered, and the stream opens.
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
	world.close().await;
}
