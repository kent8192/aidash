//! Deferred capability exposure (`deferred@1`) through the real Harness: a
//! scripted provider discovers, loads and invokes capabilities while every
//! request it receives is recorded for inspection.
use super::*;
use aidash_server::{
	domain::{Run, RunControl, RunPhase},
	harness::Harness,
};
use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
use std::collections::BTreeSet;
use tokio::sync::Mutex;

const DEFERRED: &str = "2.0.0";
const LEGACY: &str = "2.0.1";
const METADATA_BYTES: usize = 16384;
const SCHEMA_BYTES: usize = 8192;
const SKILL_BYTES: usize = 2048;
const MANDATORY: [&str; 7] = [
	"capability_describe",
	"capability_load",
	"capability_search",
	"capability_unload",
	"human_request",
	"skill_asset_read",
	"workspace_read",
];
const EAGER: &str = "plugin_0";
const GUIDE_BODY: &str = "GUIDE-BODY: Draft the release notes from the checklist.";
const ATLAS_BODY: &str = "ATLAS-BODY:";
const DIRECT_BODY: &str = "Load this only when selected";
const LONG_FILE: &str = "αβγδεζηθικλμνξοπρστυφχψω";
const INDEX_HEADER: &str = "Discoverable capabilities that are not loaded";
const CATCH_UP: &str = "Run-message catch-up";

enum Reply {
	Call(&'static str, Value),
	Text(&'static str),
}
type Script = Box<dyn FnMut(&Value, &Value) -> Reply + Send>;

struct Deferred {
	c: CoreFixture,
	server: tokio::task::JoinHandle<()>,
	requests: Arc<Mutex<Vec<Value>>>,
	script: Arc<Mutex<Script>>,
	/// 1-based request numbers the provider rejects with a retryable 503.
	unavailable: Arc<Mutex<BTreeSet<usize>>>,
}

#[rstest::fixture]
async fn deferred_fixture(#[future] test_environment: Arc<TestEnvironment>) -> Deferred {
	let env = test_environment.await;
	let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
	let script: Arc<Mutex<Script>> = Arc::new(Mutex::new(Box::new(|_: &Value, _: &Value| {
		Reply::Text("No script.")
	})));
	let unavailable = Arc::new(Mutex::new(BTreeSet::new()));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let (capture, scripted, failing) = (requests.clone(), script.clone(), unavailable.clone());
	let router = Router::new().route(
		"/v1/chat/completions",
		post(move |Json(body): Json<Value>| {
			let (capture, scripted, failing) = (capture.clone(), scripted.clone(), failing.clone());
			async move {
				let context: Value =
					serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
				let count = {
					let mut requests = capture.lock().await;
					requests.push(body.clone());
					requests.len()
				};
				if failing.lock().await.contains(&count) {
					return (
						StatusCode::SERVICE_UNAVAILABLE,
						Json(json!({"error":{"message":"provider temporarily unavailable"}})),
					)
						.into_response();
				}
				let reply = {
					let mut script = scripted.lock().await;
					(*script)(&body, &context)
				};
				let message = match reply {
					Reply::Call(name, arguments) => {
						json!({"role":"assistant","content":null,"tool_calls":[{"id":format!("deferred-{count}"),"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]})
					}
					Reply::Text(text) => json!({"role":"assistant","content":text}),
				};
				let finish = if message.get("tool_calls").is_some() {
					"tool_calls"
				} else {
					"stop"
				};
				Json(
					json!({"choices":[{"index":0,"finish_reason":finish,"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}}),
				)
				.into_response()
			}
		}),
	);
	let server = tokio::spawn(async move {
		axum::serve(listener, router).await.unwrap();
	});
	let mut c = build_core_fixture_at(env, "aidash://deferred", &endpoint).await;
	let atlas = format!(
		"{ATLAS_BODY} {}",
		"Map every dependency before the release. ".repeat(36)
	);
	for (id, name, description, instructions, files) in [
		(
			"guide-skill",
			"Release guide",
			"Prepare release notes from a checklist",
			GUIDE_BODY.to_owned(),
			json!([{"path":"references/long.md","content":LONG_FILE}]),
		),
		(
			"atlas-skill",
			"Dependency atlas",
			"Map the dependencies of a release",
			atlas,
			json!([]),
		),
	] {
		let entry = json!({"id":id,"version":"1.0.0","kind":"skill","name":{"en":name},"description":{"en":description},"schema":{},"config":{"instructions":instructions,"files":files}});
		register_approved(&c, entry).await;
	}
	for (version, deferred) in [(DEFERRED, true), (LEGACY, false)] {
		register_agent(&c, version, deferred).await;
		c.policy["subjects"]
			[aidash_server::domain::qualified_agent(&c.f.config.node_id, "research", version)] =
			json!({"kind":"agent"});
	}
	let (status, result) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":c.policy}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	Deferred {
		c,
		server,
		requests,
		script,
		unavailable,
	}
}

async fn register_approved(c: &CoreFixture, entry: Value) {
	let (status, result) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/registry",
		entry.clone(),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	let (status, result) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":entry["id"],"version":entry["version"]},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
}

/// Twin Agents with identical Bindings; only the deferred one has an
/// Exposure policy and an Eager binding.
async fn register_agent(c: &CoreFixture, version: &str, deferred: bool) {
	let node = &c.f.config.node_id;
	let target = |id: &str| json!({"registry_node":node,"id":id,"version":"1.0.0"});
	let mut plugin = json!({"kind":"tool","target":target("http"),"alias":EAGER,"narrow":{}});
	if deferred {
		plugin["exposure"] = json!("eager");
	}
	let mut agent = c.f.registry.get("research", "1.1.0").await.unwrap();
	agent.version = version.into();
	agent.binding_normalization = None;
	agent.config["instructions"] = json!("Follow the scripted capability journey.");
	agent.config["bindings"] = json!([
		plugin,
		{"kind":"skill","target":target("guide-skill"),"narrow":{}},
		{"kind":"skill","target":target("atlas-skill"),"narrow":{}},
		{"kind":"source","target":target("fixture-skills"),"narrow":{}},
	]);
	agent.config["remove_default"] = json!([]);
	agent.config["max_steps"] = json!(80);
	if deferred {
		agent.config["exposure"] = json!({"version":"deferred@1","metadata_bytes":METADATA_BYTES,"schema_bytes":SCHEMA_BYTES,"skill_bytes":SKILL_BYTES});
	}
	register_approved(c, json!(agent)).await;
}

impl Deferred {
	/// Scripts are set before delegation, while no provider request is in flight.
	fn script(&self, script: impl FnMut(&Value, &Value) -> Reply + Send + 'static) {
		*self
			.script
			.try_lock()
			.expect("no provider request is in flight") = Box::new(script);
	}
	async fn requests(&self) -> Vec<Value> {
		self.requests.lock().await.clone()
	}
	async fn delegate(&self, task: Uuid, version: &str) -> Run {
		let (status, result) = request(
			&self.c.app,
			&self.c.token,
			"POST",
			&format!("/api/tasks/{task}/delegate"),
			json!({"node_id":self.c.f.config.node_id,"agent":{"id":"research","version":version}}),
		)
		.await;
		assert_eq!(status, 200, "{result}");
		self.c
			.f
			.store
			.runs()
			.await
			.unwrap()
			.into_iter()
			.find(|run| run.task_id == task)
			.unwrap()
	}
	async fn task(&self, workspace: Uuid) -> Uuid {
		let (status, task) = request(
			&self.c.app,
			&self.c.token,
			"POST",
			&format!("/api/workspaces/{workspace}/tasks"),
			json!({"title":"Second research","description":"Use approved tools"}),
		)
		.await;
		assert_eq!(status, 200, "{task}");
		serde_json::from_value(task["id"].clone()).unwrap()
	}
	/// One durable step on a fresh Harness: nothing survives in memory between steps.
	async fn step(&self) {
		Harness {
			federation: self.c.f.clone(),
		}
		.worker_once()
		.await
		.unwrap();
	}
	async fn run(&self, id: Uuid) -> Run {
		self.c.f.store.run(id).await.unwrap()
	}
	/// Step until the Run completes, fails, is cancelled or is paused.
	async fn drive(&self, id: Uuid) -> Run {
		let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
		loop {
			self.step().await;
			let run = self.run(id).await;
			if settled(&run) {
				return run;
			}
			assert!(
				tokio::time::Instant::now() < deadline,
				"run stalled: phase={} error={:?} state={:?}",
				run.phase(),
				run.error,
				run.state
			);
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	}
	async fn close(self) {
		self.server.abort();
		let _ = self.server.await;
		self.c.close().await;
	}
}

fn settled(run: &Run) -> bool {
	matches!(
		run.phase(),
		RunPhase::Completed | RunPhase::Failed | RunPhase::Cancelled
	) || run.control == RunControl::Paused
}
fn completed(run: &Run) {
	assert_eq!(
		run.phase(),
		RunPhase::Completed,
		"error={:?} state={:?}",
		run.error,
		run.state
	);
}
fn tool_names(body: &Value) -> BTreeSet<String> {
	let names = body["tools"]
		.as_array()
		.into_iter()
		.flatten()
		.map(|tool| tool["function"]["name"].as_str().unwrap().to_owned())
		.collect::<Vec<_>>();
	let unique = names.iter().cloned().collect::<BTreeSet<_>>();
	assert_eq!(unique.len(), names.len(), "duplicate tool: {names:?}");
	unique
}
fn instructions(body: &Value) -> &str {
	body["messages"][0]["content"].as_str().unwrap()
}
fn expected_initial() -> BTreeSet<String> {
	MANDATORY
		.iter()
		.chain(&[EAGER])
		.map(|name| (*name).to_owned())
		.collect()
}
/// `(call, result)` of every tool event in a model-visible or durable context.
fn events(context: &Value) -> Vec<(&Value, &Value)> {
	context["history"]
		.as_array()
		.into_iter()
		.flatten()
		.filter(|event| event["kind"] == "tool")
		.map(|event| (&event["call"], &event["result"]))
		.collect()
}
fn results<'a>(context: &'a Value, name: &str) -> Vec<&'a Value> {
	events(context)
		.into_iter()
		.filter(|(call, _)| call["name"] == name)
		.map(|(_, result)| result)
		.collect()
}
fn durable(run: &Run) -> Value {
	serde_json::to_value(&run.context).unwrap()
}
/// Search results of every page so far, in order.
fn found(context: &Value) -> Vec<Value> {
	results(context, "capability_search")
		.into_iter()
		.flat_map(|page| page["results"].as_array().cloned().unwrap_or_default())
		.collect()
}
fn skill_named(context: &Value, name: &str) -> Vec<Value> {
	found(context)
		.into_iter()
		.filter(|result| result["kind"] == "skill" && result["name"] == name)
		.collect()
}
/// Page through every Discoverable capability, then describe capabilities of
/// each kind until their definitions exceed that kind's budget.
fn discover(context: &Value) -> Option<Reply> {
	let events = events(context);
	let pages = events
		.iter()
		.filter(|(call, _)| {
			call["name"] == "capability_search" && call["arguments"].get("query").is_none()
		})
		.map(|(_, result)| *result)
		.collect::<Vec<_>>();
	match pages.last() {
		None => return Some(Reply::Call("capability_search", json!({}))),
		Some(page) if page["next_cursor"].is_string() => {
			return Some(Reply::Call(
				"capability_search",
				json!({"cursor":page["next_cursor"]}),
			));
		}
		_ => {}
	}
	let described = events
		.iter()
		.filter(|(call, _)| call["name"] == "capability_describe")
		.map(|(_, result)| *result)
		.collect::<Vec<_>>();
	let full = |kind: &str, budget: usize| {
		described
			.iter()
			.filter(|detail| detail["kind"] == kind)
			.filter_map(|detail| detail["bytes"].as_u64())
			.sum::<u64>() as usize
			> budget
	};
	let (tools_full, skills_full) = (full("tool", SCHEMA_BYTES), full("skill", SKILL_BYTES));
	pages
		.iter()
		.flat_map(|page| page["results"].as_array().into_iter().flatten())
		.filter(|result| match result["kind"].as_str() {
			Some("tool") => !tools_full,
			Some("skill") => !skills_full,
			_ => false,
		})
		.filter_map(|result| result["alias"].as_str())
		.find(|alias| !described.iter().any(|detail| detail["alias"] == *alias))
		.map(|alias| Reply::Call("capability_describe", json!({"alias":alias})))
}
async fn skill_record(c: &CoreFixture) -> String {
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
	let records: Vec<Value> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("data"))
			.from(Alias::new("core_records"))
			.and_where(Expr::col(Alias::new("kind")).eq("skills"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(c.f.store.pool.driver())
	.await
	.unwrap();
	assert!(!records.is_empty(), "direct Skills are pinned at admission");
	Value::Array(records).to_string()
}

#[rstest::rstest]
#[tokio::test]
async fn deferred_first_request_exposes_only_mandatory_and_eager_capabilities(
	#[future] deferred_fixture: Deferred,
) {
	let d = Box::pin(deferred_fixture).await;
	d.script(|_, context| discover(context).unwrap_or(Reply::Text("Discovery complete.")));
	let run = d.delegate(d.c.task, DEFERRED).await;
	let finished = d.drive(run.id).await;
	completed(&finished);
	let requests = d.requests().await;
	let first = &requests[0];
	let initial = expected_initial();
	assert_eq!(tool_names(first), initial);
	let text = instructions(first);
	for body in [GUIDE_BODY, ATLAS_BODY, DIRECT_BODY] {
		assert!(!text.contains(body), "Skill body {body} is not resident");
	}
	assert!(text.contains(INDEX_HEADER), "{text}");
	assert!(
		!text.contains("\"properties\"") && !text.contains("\"parameters\""),
		"no deferred schema in the instructions: {text}"
	);
	let context = durable(&finished);
	let described = results(&context, "capability_describe");
	let discovered = found(&context);
	assert!(
		described.len() < discovered.len(),
		"budgets are exceeded early"
	);
	assert!(
		discovered.len() > 16,
		"search paged through every capability"
	);
	let bytes = |kind: &str, exposed: Option<bool>| {
		described
			.iter()
			.filter(|detail| detail["kind"] == kind)
			.filter(|detail| {
				exposed.is_none_or(|exposed| {
					initial.contains(detail["alias"].as_str().unwrap()) == exposed
				})
			})
			.map(|detail| detail["bytes"].as_u64().unwrap() as usize)
			.sum::<usize>()
	};
	assert!(
		bytes("tool", None) > SCHEMA_BYTES,
		"bound Tools exceed schema_bytes"
	);
	assert!(
		bytes("skill", None) > SKILL_BYTES,
		"bound Skills exceed skill_bytes"
	);
	assert!(bytes("tool", Some(true)) <= SCHEMA_BYTES);
	for detail in &described {
		let alias = detail["alias"].as_str().unwrap();
		assert!(detail["error"].is_null(), "{detail}");
		if detail["kind"] == "tool" && !initial.contains(alias) {
			assert!(
				!text.contains(&detail["detail"]["parameters"].to_string()),
				"deferred schema of {alias} is not in the instructions"
			);
			assert_eq!(detail["budget"], SCHEMA_BYTES);
		}
	}
	// Every Skill is discoverable; same-name direct Skills keep distinct identities.
	let direct = skill_named(&context, "analysis");
	assert_eq!(direct.len(), 2, "{direct:?}");
	assert_ne!(direct[0]["alias"], direct[1]["alias"]);
	assert_eq!(skill_named(&context, "Release guide").len(), 1);
	assert_eq!(skill_named(&context, "Dependency atlas").len(), 1);
	// Discovery never loads anything.
	assert!(finished.context.exposure.is_empty());
	let usage = finished.context.usage.as_ref().unwrap();
	let exposure = usage.exposure.as_ref().unwrap();
	assert!(exposure.schema_bytes <= SCHEMA_BYTES && exposure.skill_bytes == 0);
	assert_eq!(
		exposure
			.exposed
			.iter()
			.map(|(alias, _)| alias.clone())
			.collect::<BTreeSet<_>>(),
		initial
	);
	d.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn deferred_journey_searches_describes_loads_and_invokes_a_tool(
	#[future] deferred_fixture: Deferred,
) {
	let d = Box::pin(deferred_fixture).await;
	d.script(|_, context| {
		let events = events(context);
		match events.len() {
			0 => Reply::Call("capability_search", json!({"query":"file search"})),
			1 => Reply::Call("capability_describe", json!({"alias":"file_search"})),
			2 => Reply::Call(
				"capability_load",
				json!({"alias":"file_search","digest":events[1].1["digest"]}),
			),
			3 => Reply::Call(
				"file_search",
				json!({"query":"東京","mode":"literal","scope":"working"}),
			),
			_ => Reply::Text("Searched the working files."),
		}
	});
	let run = d.delegate(d.c.task, DEFERRED).await;
	let finished = d.drive(run.id).await;
	completed(&finished);
	let requests = d.requests().await;
	assert_eq!(requests.len(), 5);
	for (index, body) in requests.iter().enumerate() {
		let names = tool_names(body);
		assert!(expected_initial().is_subset(&names));
		assert_eq!(
			names.contains("file_search"),
			index >= 3,
			"request {index} advertises {names:?}"
		);
	}
	let context = durable(&finished);
	let search = found(&context);
	let hit = search
		.iter()
		.find(|result| result["alias"] == "file_search")
		.unwrap();
	assert_eq!(hit["kind"], "tool");
	assert_eq!(hit["loaded"], false);
	let described = results(&context, "capability_describe")[0];
	assert_eq!(described["digest"], hit["digest"]);
	assert_eq!(described["detail"]["name"], "file_search");
	assert!(described["identity"]["registry"].is_object(), "{described}");
	let loaded = results(&context, "capability_load")[0];
	assert_eq!(loaded["status"], "loaded", "{loaded}");
	assert_eq!(loaded["identity"], described["identity"]);
	let searched = results(&context, "file_search")[0];
	assert!(
		searched["error"].is_null() && searched["status"] != "blocked",
		"{searched}"
	);
	assert!(searched["matches"].is_array(), "{searched}");
	let state = serde_json::to_value(&finished.context.exposure).unwrap();
	assert_eq!(state["loaded"].as_array().unwrap().len(), 1, "{state}");
	assert_eq!(state["loaded"][0]["alias"], "file_search");
	assert_eq!(state["loaded"][0]["identity"], described["identity"]);
	assert_eq!(state["loaded"][0]["digest"], described["digest"]);
	let exposure = finished
		.context
		.usage
		.as_ref()
		.unwrap()
		.exposure
		.as_ref()
		.unwrap();
	assert!(
		exposure
			.exposed
			.contains(&("file_search".into(), hit["digest"].as_str().unwrap().into()))
	);
	d.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn deferred_invocation_rejections_keep_unloaded_capabilities_unavailable(
	#[future] deferred_fixture: Deferred,
) {
	let d = Box::pin(deferred_fixture).await;
	d.script(|_, context| match events(context).len() {
		0 => Reply::Call("capability_describe", json!({"alias":"no_such_capability"})),
		1 => Reply::Call(
			"capability_load",
			json!({"alias":"no_such_capability","digest":"sha256:0"}),
		),
		2 => Reply::Call(
			"file_read",
			json!({"file_id":Uuid::nil(),"representation":"text"}),
		),
		3 => Reply::Call("skill_load", json!({"skill_id":Uuid::nil()})),
		4 => Reply::Call("capability_unload", json!({"alias":"workspace_read"})),
		5 => Reply::Call(
			"capability_load",
			json!({"alias":"file_search","digest":"sha256:stale"}),
		),
		_ => Reply::Text("Every forged call was rejected."),
	});
	let run = d.delegate(d.c.task, DEFERRED).await;
	let finished = d.drive(run.id).await;
	completed(&finished);
	let context = durable(&finished);
	let outcomes = events(&context)
		.into_iter()
		.map(|(call, result)| (call["name"].as_str().unwrap(), result.clone()))
		.collect::<Vec<_>>();
	assert_eq!(outcomes.len(), 6, "{outcomes:?}");
	let error = |index: usize| outcomes[index].1["error"].as_str().unwrap_or_default();
	assert_eq!(error(0), "UNKNOWN_CAPABILITY", "{outcomes:?}");
	assert_eq!(error(1), "UNKNOWN_CAPABILITY", "{outcomes:?}");
	assert_eq!(
		error(2),
		"capability file_read is not loaded; use capability_load"
	);
	assert_eq!(error(3), "unavailable tool skill_load");
	assert_eq!(error(4), "MANDATORY_EXPOSURE");
	assert!(error(5).contains("CAPABILITY_CHANGED"), "{outcomes:?}");
	assert!(finished.context.exposure.is_empty());
	for body in d.requests().await {
		let names = tool_names(&body);
		assert_eq!(names, expected_initial());
	}
	d.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn deferred_revoked_binding_fails_the_step_like_legacy(#[future] deferred_fixture: Deferred) {
	let d = Box::pin(deferred_fixture).await;
	d.script(|_, context| {
		if events(context).is_empty() {
			Reply::Call(
				"workspace_read",
				json!({"kind":"task","id":context["current"]["task"]["id"]}),
			)
		} else {
			Reply::Text("Read the task.")
		}
	});
	let first = d.delegate(d.c.task, DEFERRED).await;
	let second = d.task(first.workspace_id).await;
	let mut outcomes = vec![];
	for (revision, (task, version)) in [(d.c.task, DEFERRED), (second, LEGACY)]
		.into_iter()
		.enumerate()
	{
		let run = if version == DEFERRED {
			first.clone()
		} else {
			d.delegate(task, version).await
		};
		let sent = d.requests().await.len();
		let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
		while d.run(run.id).await.context.history.is_empty() {
			d.step().await;
			assert!(tokio::time::Instant::now() < deadline);
		}
		assert_eq!(d.requests().await.len(), sent + 1);
		// Withdraw the catalog approval of the bound external Tool. Under
		// deferred@1 it is Eager here, and the step still fails before inference.
		let (status, result) = request(
			&d.c.app,
			&d.c.f.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"http","version":"1.0.0"},"expected_revision":1 + 2 * revision,"enabled":false}),
		)
		.await;
		assert_eq!(status, 200, "{result}");
		let failed = d.drive(run.id).await;
		assert_eq!(
			d.requests().await.len(),
			sent + 1,
			"no inference after revocation"
		);
		outcomes.push((
			failed.phase().to_string(),
			failed.control,
			failed.error.clone(),
		));
		let (status, result) = request(
			&d.c.app,
			&d.c.f.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"http","version":"1.0.0"},"expected_revision":2 + 2 * revision,"enabled":true}),
		)
		.await;
		assert_eq!(status, 200, "{result}");
	}
	assert_ne!(
		outcomes[0].0,
		RunPhase::Completed.to_string(),
		"{outcomes:?}"
	);
	assert_eq!(
		outcomes[0], outcomes[1],
		"deferred revocation matches legacy"
	);
	d.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn deferred_discovery_never_reveals_foreign_unbound_or_unapproved_entries(
	#[future] deferred_fixture: Deferred,
) {
	let d = Box::pin(deferred_fixture).await;
	let node = d.c.f.config.node_id.clone();
	let zebra = |id: &str| json!({"id":id,"version":"1.0.0","kind":"tool","name":{"en":"Zebra ledger"},"description":{"en":"Zebra ledger secret tool"},"capabilities":[],"languages":["en"],"schema":{"type":"object"},"config":{"registry_node":node,"provider":"integration.http@1","operation":"invoke","default_alias":id.replace('-', "_"),"tier":"integration","transport":{"transport":"http","endpoint":"http://127.0.0.1:9/effect","credential_env":null,"replay":"idempotent"}}});
	// Approved for this tenant but never bound.
	register_approved(&d.c, zebra("zebra-unbound")).await;
	// Registered on this Node but approved nowhere.
	let (status, result) = request(
		&d.c.app,
		&d.c.f.config.api_token,
		"POST",
		"/api/registry",
		zebra("zebra-unapproved"),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	// Approved only in another tenant's catalog.
	let (status, result) = request(
		&d.c.app,
		&d.c.f.config.api_token,
		"POST",
		"/api/registry",
		zebra("zebra-foreign"),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	let foreign = json!({"tenant":"globex","subjects":{"carol":{"kind":"user"}},"policies":[{"id":"globex-work","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]});
	let (status, result) = request(
		&d.c.app,
		&d.c.f.config.api_token,
		"POST",
		"/api/authorization/globex",
		json!({"expected_revision":0,"bundle":foreign}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	let (status, result) = request(
		&d.c.app,
		&d.c.f.config.api_token,
		"POST",
		"/api/authorization/globex/catalog",
		json!({"entry":{"id":"zebra-foreign","version":"1.0.0"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	const FORGED: [&str; 5] = [
		"zebra_unbound",
		"zebra_unapproved",
		"zebra_foreign",
		"shell",
		"code_interpreter",
	];
	d.script(|_, context| {
		if let Some(reply) = discover(context) {
			return reply;
		}
		let events = events(context);
		if !events
			.iter()
			.any(|(call, _)| call["arguments"]["query"] == "zebra ledger")
		{
			return Reply::Call("capability_search", json!({"query":"zebra ledger"}));
		}
		let tried = events
			.iter()
			.filter_map(|(call, _)| call["arguments"]["alias"].as_str())
			.collect::<BTreeSet<_>>();
		match FORGED.iter().find(|alias| !tried.contains(**alias)) {
			Some(alias) => Reply::Call("capability_describe", json!({"alias":alias})),
			None => Reply::Text("Discovery is isolated."),
		}
	});
	let run = d.delegate(d.c.task, DEFERRED).await;
	let finished = d.drive(run.id).await;
	completed(&finished);
	let context = durable(&finished);
	let leaked = |value: &Value| {
		let text = value.to_string().to_lowercase();
		text.contains("zebra")
			|| ["shell", "code_interpreter", "outbound_get", "file_share"]
				.iter()
				.any(|alias| text.contains(&format!("\"alias\":\"{alias}\"")))
	};
	for (call, result) in events(&context) {
		match call["name"].as_str().unwrap() {
			"capability_search" => assert!(!leaked(result), "{result}"),
			"capability_describe"
				if FORGED.contains(&call["arguments"]["alias"].as_str().unwrap()) =>
			{
				assert_eq!(result["error"], "UNKNOWN_CAPABILITY", "{result}");
			}
			"capability_describe" => assert!(!leaked(result), "{result}"),
			name => panic!("unexpected call {name}"),
		}
	}
	let zebra_page = events(&context)
		.into_iter()
		.find(|(call, _)| call["arguments"]["query"] == "zebra ledger")
		.unwrap()
		.1;
	assert_eq!(zebra_page["results"], json!([]), "{zebra_page}");
	for body in d.requests().await {
		let text = instructions(&body);
		assert!(text.contains(INDEX_HEADER));
		assert!(
			!text.contains("more — use capability_search"),
			"the whole index is visible"
		);
		assert!(!text.to_lowercase().contains("zebra"), "{text}");
		for alias in ["shell", "code_interpreter", "outbound_get", "file_share"] {
			assert!(!text.contains(&format!("\n{alias} [tool]")), "{text}");
		}
		assert!(tool_names(&body).iter().all(|name| !name.contains("zebra")));
	}
	d.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn deferred_skills_load_once_and_read_assets_in_chunks(#[future] deferred_fixture: Deferred) {
	let d = Box::pin(deferred_fixture).await;
	d.script(|_, context| {
		let events = events(context);
		let guide = skill_named(context, "Release guide");
		let direct = skill_named(context, "analysis");
		let load = |skill: &Value| {
			Reply::Call(
				"capability_load",
				json!({"alias":skill["alias"],"digest":skill["digest"]}),
			)
		};
		let read = |skill: &Value, extra: Value| {
			let mut input = json!({"alias":skill["alias"],"digest":skill["digest"]});
			input
				.as_object_mut()
				.unwrap()
				.extend(extra.as_object().unwrap().clone());
			Reply::Call("skill_asset_read", input)
		};
		match events.len() {
			0 => Reply::Call("capability_search", json!({"query":"release guide"})),
			1 | 2 => load(&guide[0]),
			3 => Reply::Call("capability_search", json!({"query":"analysis"})),
			4 | 5 => load(&direct[0]),
			6 => read(
				&guide[0],
				json!({"path":"references/long.md","max_chars":5}),
			),
			7 => read(
				&guide[0],
				json!({"path":"references/long.md","offset":events[6].1["next_offset"]}),
			),
			8 => read(
				&direct[0],
				json!({"path":"references/guide.md","max_chars":1}),
			),
			9 => read(&direct[0], json!({"path":"../scripts/analyze.py"})),
			_ => Reply::Text("Read the Skill assets."),
		}
	});
	let run = d.delegate(d.c.task, DEFERRED).await;
	let finished = d.drive(run.id).await;
	completed(&finished);
	let context = durable(&finished);
	let guide = &skill_named(&context, "Release guide")[0];
	let direct = skill_named(&context, "analysis");
	assert_eq!(direct.len(), 2);
	assert_ne!(
		direct[0]["alias"], direct[1]["alias"],
		"same-name Skills keep distinct identities"
	);
	let loads = results(&context, "capability_load");
	assert_eq!(
		loads
			.iter()
			.map(|result| result["status"].as_str().unwrap_or_default())
			.collect::<Vec<_>>(),
		["loaded", "already_loaded", "loaded", "already_loaded"],
		"{loads:?}"
	);
	for repeated in [loads[1], loads[3]] {
		assert!(repeated.get("exposure_update").is_none());
	}
	assert!(
		loads
			.iter()
			.all(|result| !result.to_string().contains(GUIDE_BODY)
				&& !result.to_string().contains(DIRECT_BODY)),
		"bodies are resident, never returned"
	);
	let reads = results(&context, "skill_asset_read");
	assert_eq!(reads[0]["content"], "αβγδε", "{}", reads[0]);
	assert_eq!(reads[0]["next_offset"], 5);
	assert_eq!(reads[0]["truncated"], true);
	assert_eq!(
		reads[1]["content"],
		&LONG_FILE[LONG_FILE.char_indices().nth(5).unwrap().0..]
	);
	assert_eq!(reads[1]["offset"], 5);
	assert!(reads[1]["next_offset"].is_null());
	assert_eq!(reads[1]["truncated"], false);
	assert_eq!(reads[2]["content"], "東", "{}", reads[2]);
	assert_eq!(reads[2]["next_offset"], 1);
	assert_eq!(reads[2]["truncated"], true);
	assert!(reads[3]["error"].is_string(), "path escape: {}", reads[3]);
	let block = |alias: &Value| format!("\nSkill {} (", alias.as_str().unwrap());
	for (index, body) in d.requests().await.iter().enumerate() {
		let text = instructions(body);
		// Request n follows n tool results: the guide is loaded by result 2 and
		// the direct Skill by result 5.
		let (guide_resident, direct_resident) = (index >= 2, index >= 5);
		assert_eq!(
			text.matches(GUIDE_BODY).count(),
			usize::from(guide_resident)
		);
		assert_eq!(
			text.matches(&block(&guide["alias"])).count(),
			usize::from(guide_resident)
		);
		assert_eq!(
			text.matches(DIRECT_BODY).count(),
			usize::from(direct_resident)
		);
		assert_eq!(
			text.matches(&block(&direct[0]["alias"])).count(),
			usize::from(direct_resident)
		);
		assert!(!text.contains(ATLAS_BODY));
		assert_eq!(
			text.contains(&format!("\n{} [skill]", guide["alias"].as_str().unwrap())),
			!guide_resident,
			"a resident Skill leaves the index"
		);
	}
	let state = serde_json::to_value(&finished.context.exposure).unwrap();
	let loaded = state["loaded"].as_array().unwrap();
	assert_eq!(loaded.len(), 2, "{state}");
	assert_eq!(loaded[0]["alias"], guide["alias"]);
	assert!(loaded[0]["identity"]["registry"].is_object(), "{state}");
	assert_eq!(loaded[1]["alias"], direct[0]["alias"]);
	assert!(loaded[1]["identity"]["direct_skill"].is_object(), "{state}");
	assert!(
		!skill_record(&d.c).await.contains("\"loaded\":true"),
		"deferred@1 never writes the Skill record's loaded flag"
	);
	d.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn deferred_exposure_survives_fresh_harness_instances_without_duplication(
	#[future] deferred_fixture: Deferred,
) {
	let d = Box::pin(deferred_fixture).await;
	d.script(|_, context| {
		let events = events(context);
		let tool = found(context)
			.into_iter()
			.find(|result| result["alias"] == "file_search");
		let guide = skill_named(context, "Release guide");
		match events.len() {
			0 => Reply::Call("capability_search", json!({"query":"file search"})),
			1 => Reply::Call(
				"capability_load",
				json!({"alias":"file_search","digest":tool.unwrap()["digest"]}),
			),
			2 => Reply::Call("capability_search", json!({"query":"release guide"})),
			3 => Reply::Call(
				"capability_load",
				json!({"alias":guide[0]["alias"],"digest":guide[0]["digest"]}),
			),
			4 => Reply::Call(
				"file_search",
				json!({"query":"release","mode":"literal","scope":"working"}),
			),
			_ => Reply::Text("Finished after restarts."),
		}
	});
	let run = d.delegate(d.c.task, DEFERRED).await;
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
	let mut seen: Vec<Value> = vec![];
	let finished = loop {
		// Each step runs on a new Harness and reloads the Run from Postgres.
		d.step().await;
		let current = d.run(run.id).await;
		let loaded = serde_json::to_value(&current.context.exposure).unwrap()["loaded"]
			.as_array()
			.cloned()
			.unwrap_or_default();
		assert!(
			loaded.starts_with(&seen),
			"persisted identities never change: {seen:?} -> {loaded:?}"
		);
		seen = loaded;
		if settled(&current) {
			break current;
		}
		assert!(tokio::time::Instant::now() < deadline);
	};
	completed(&finished);
	assert_eq!(
		seen.iter()
			.map(|loaded| loaded["kind"].as_str().unwrap())
			.collect::<Vec<_>>(),
		["tool", "skill"]
	);
	let requests = d.requests().await;
	assert_eq!(requests.len(), 6);
	for (index, body) in requests.iter().enumerate() {
		let text = instructions(body);
		assert_eq!(text.matches("Additional user instructions:").count(), 1);
		assert_eq!(text.matches(INDEX_HEADER).count(), 1);
		assert_eq!(
			text.matches("Follow the scripted capability journey.")
				.count(),
			1
		);
		assert_eq!(text.matches(GUIDE_BODY).count(), usize::from(index >= 4));
		assert_eq!(tool_names(body).contains("file_search"), index >= 2);
	}
	let searched = results(&durable(&finished), "file_search")[0].clone();
	assert!(searched["error"].is_null(), "{searched}");
	d.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn deferred_run_message_catch_up_advertises_only_workspace_read(
	#[future] deferred_fixture: Deferred,
) {
	let d = Box::pin(deferred_fixture).await;
	d.script(|body, context| {
		if !instructions(body).contains(CATCH_UP) {
			return Reply::Text("Done.");
		}
		let current = &context["current"];
		let required = current["run_messages"]
			.as_array()
			.into_iter()
			.flatten()
			.filter(|entry| entry["requires_workspace_read"] == true)
			.map(|entry| entry["record"]["id"].clone())
			.chain(
				current["deferred_run_message_reads"]
					.as_array()
					.into_iter()
					.flatten()
					.cloned(),
			)
			.collect::<Vec<_>>();
		let events = events(context);
		for id in required {
			let last = events.iter().rev().find(|(call, _)| {
				call["name"] == "workspace_read" && call["arguments"]["id"] == id
			});
			match last {
				None => {
					return Reply::Call("workspace_read", json!({"kind":"message","id":id}));
				}
				Some((_, result)) if result["next_offset"].is_u64() => {
					return Reply::Call(
						"workspace_read",
						json!({"kind":"message","id":id,"offset":result["next_offset"]}),
					);
				}
				Some(_) => {}
			}
		}
		Reply::Text("The user sent two corrections about the release.")
	});
	let run = d.delegate(d.c.task, DEFERRED).await;
	let limit = d.c.f.run_message_limit(&run).await.unwrap();
	for content in [
		"Please include the rollback steps.",
		"Also name the release owner.",
	] {
		let key = format!("human:{}:{}", run.id, Uuid::new_v4());
		d.c.f
			.admit_run_message(&run, "human", content, &key, limit)
			.await
			.unwrap();
		d.c.f.deliver_run_messages(&run).await.unwrap();
	}
	// An earlier summary turns every later run message into catch-up.
	let summarized = d.c.f.store.run_inputs(run.id).await.unwrap()[0].seq;
	let worker = Uuid::new_v4();
	let mut leased = d.c.f.store.lease_run(worker, 30).await.unwrap().unwrap();
	assert_eq!(leased.id, run.id);
	leased.context.run_message_summary = "The user wants a release checklist.".into();
	leased.context.run_message_summary_seq = summarized;
	leased.observed_input_seq = summarized;
	d.c.f
		.store
		.save_run(&leased, worker, "run.message_received")
		.await
		.unwrap();
	d.c.f.store.release_lease(run.id, worker).await.unwrap();
	let finished = d.drive(run.id).await;
	completed(&finished);
	let requests = d.requests().await;
	let (catch_up, ordinary): (Vec<_>, Vec<_>) = requests
		.iter()
		.partition(|body| instructions(body).contains(CATCH_UP));
	assert!(!catch_up.is_empty() && !ordinary.is_empty(), "{requests:?}");
	for body in &catch_up {
		assert_eq!(
			tool_names(body),
			BTreeSet::from(["workspace_read".to_owned()])
		);
	}
	for body in &ordinary {
		let names = tool_names(body);
		assert!(names.contains("human_request") && names.contains("workspace_read"));
		assert_eq!(names, expected_initial());
	}
	d.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn legacy_agent_beside_deferred_keeps_legacy_tools_and_skill_record(
	#[future] deferred_fixture: Deferred,
) {
	let d = Box::pin(deferred_fixture).await;
	d.script(|_, context| {
		let list = results(context, "skill_list");
		match events(context).len() {
			0 => Reply::Call("skill_list", json!({})),
			1 => Reply::Call(
				"skill_load",
				json!({"skill_id":list[0]["skills"][0]["skill_id"],"expected_digest":list[0]["skills"][0]["digest"]}),
			),
			_ => Reply::Text("Loaded the legacy Skill."),
		}
	});
	let run = d.delegate(d.c.task, LEGACY).await;
	let finished = d.drive(run.id).await;
	completed(&finished);
	let requests = d.requests().await;
	assert_eq!(requests.len(), 3);
	for body in &requests {
		let names = tool_names(body);
		for name in [
			"skill_list",
			"skill_load",
			"skill_read",
			"workspace_read",
			"human_request",
			"file_search",
			"file_read",
			EAGER,
		] {
			assert!(names.contains(name), "missing {name}: {names:?}");
		}
		assert!(
			names
				.iter()
				.all(|name| !name.starts_with("capability_") && name != "skill_asset_read"),
			"{names:?}"
		);
		let text = instructions(body);
		assert!(text.contains("Skill guide-skill@1.0.0:") && text.contains(GUIDE_BODY));
		assert!(text.contains(ATLAS_BODY));
		assert!(text.contains("Pinned Skills"));
		assert!(!text.contains(INDEX_HEADER));
	}
	let context = durable(&finished);
	let loaded = results(&context, "skill_load")[0];
	assert!(
		loaded["content"]
			.as_str()
			.is_some_and(|content| content.contains(DIRECT_BODY)),
		"{loaded}"
	);
	assert!(
		context.get("exposure").is_none(),
		"legacy Runs have no Exposure set"
	);
	assert!(finished.context.usage.as_ref().unwrap().exposure.is_none());
	assert!(
		skill_record(&d.c).await.contains("\"loaded\":true"),
		"legacy skill_load still writes the Skill record"
	);
	d.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn deferred_skill_source_reenters_thinking_after_a_retryable_provider_error(
	#[future] deferred_fixture: Deferred,
) {
	let d = Box::pin(deferred_fixture).await;
	// The first boundary and the boundary after the direct Skill load are each
	// retried once; the retry reuses the boundary's Source observation.
	d.unavailable.lock().await.extend([1, 4]);
	d.script(|_, context| {
		let direct = skill_named(context, "analysis");
		match events(context).len() {
			0 => Reply::Call("capability_search", json!({"query":"analysis"})),
			1 => Reply::Call(
				"capability_load",
				json!({"alias":direct[0]["alias"],"digest":direct[0]["digest"]}),
			),
			_ => Reply::Text("Loaded after retries."),
		}
	});
	let run = d.delegate(d.c.task, DEFERRED).await;
	let finished = d.drive(run.id).await;
	completed(&finished);
	let requests = d.requests().await;
	assert_eq!(requests.len(), 5, "two rejected requests were retried");
	for (failed, retried) in [(0, 1), (3, 4)] {
		assert_eq!(
			instructions(&requests[failed]),
			instructions(&requests[retried]),
			"a retry re-enters the same boundary"
		);
		assert_eq!(
			tool_names(&requests[failed]),
			tool_names(&requests[retried])
		);
	}
	assert!(!instructions(&requests[1]).contains(DIRECT_BODY));
	assert_eq!(instructions(&requests[4]).matches(DIRECT_BODY).count(), 1);
	let loaded = results(&durable(&finished), "capability_load")[0].clone();
	assert_eq!(loaded["status"], "loaded", "{loaded}");
	assert_eq!(finished.context.exposure.loaded.len(), 1);
	d.close().await;
}
