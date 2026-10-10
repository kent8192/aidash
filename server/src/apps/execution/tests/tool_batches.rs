//! Tool Batches: atomic batch admission in the invocation journal, the
//! registry contract for batching settings, and model-driven batches of real
//! core file reads.
#[path = "support/legacy.rs"]
mod common;
use aidash_application::ports::execution::ToolSlots;
use aidash_server::{
	Error,
	capabilities::{Profile, Runtime},
	context::ContextEvent,
	domain::{Run, RunPhase, RunState},
	federation::Federation,
	harness::Harness,
	store::{Invocation, Store},
};
use axum::{Json, Router, routing::post};
use common::{TestApplication, TestEnvironment, request, test_environment};
use serde_json::{Value, json};
use std::{
	collections::VecDeque,
	sync::{Arc, Mutex},
	time::{Duration, Instant},
};
use uuid::Uuid;

async fn contract_accepts(
	pool: &sqlx::PgPool,
	function: &str,
	values: Vec<serde_json::Value>,
) -> bool {
	use reinhardt::query::{
		Alias, Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	};
	let query = Query::select()
		.expr(reinhardt::query::SimpleExpr::FunctionCall(
			Alias::new(function).into_iden(),
			values
				.into_iter()
				.map(|v| match v {
					serde_json::Value::String(text) => Expr::value(text).into(),
					value => Expr::value(value).into(),
				})
				.collect(),
		))
		.to_string(PostgresQueryBuilder);
	sqlx::query_scalar(&query).fetch_one(pool).await.unwrap()
}

#[rstest::rstest]
#[tokio::test]
async fn registry_contract_bounds_tool_parallelism_and_concurrency_narrowing(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	// Arrange: a minimal valid Agent definition binding one Tool.
	let (f, url, schema) = common::setup(&environment).await;
	let pool = f.store.pool.driver().clone();
	let agent = |parallelism: Option<Value>, narrow: Value| {
		let mut config = json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Test",
			"bindings":[{"kind":"tool","target":{"registry_node":f.config.node_id,"id":"http","version":"1.0.0"},"alias":"plugin_0","narrow":narrow}]});
		if let Some(parallelism) = parallelism {
			config["tool_parallelism"] = parallelism;
		}
		vec![config]
	};
	let accepts =
		async |config| contract_accepts(&pool, "aidash_agent_bindings_is_valid", config).await;
	// Act / Assert: a definition without the field keeps its meaning.
	assert!(accepts(agent(None, json!({}))).await);
	for valid in [1, 2, 16] {
		assert!(
			accepts(agent(Some(json!(valid)), json!({}))).await,
			"{valid}"
		);
	}
	for invalid in [json!(0), json!(17), json!("2"), json!(2.5), Value::Null] {
		assert!(
			!accepts(agent(Some(invalid.clone()), json!({}))).await,
			"{invalid}"
		);
	}
	for valid in ["sequential", "shared_read"] {
		assert!(
			accepts(agent(None, json!({"concurrency":valid}))).await,
			"{valid}"
		);
	}
	for invalid in [json!("parallel"), json!(1), json!(["sequential"])] {
		assert!(
			!accepts(agent(None, json!({"concurrency":invalid.clone()}))).await,
			"{invalid}"
		);
	}
	// Only Tool and Bundle Bindings bind Tools whose concurrency can be lowered.
	let binding = |kind: &str, narrow: Value| {
		let mut config = agent(None, json!({})).remove(0);
		config["bindings"] = json!([{"kind":kind,"target":{"registry_node":f.config.node_id,"id":kind,"version":"1.0.0"},"narrow":narrow}]);
		vec![config]
	};
	for kind in ["tool", "bundle"] {
		assert!(
			accepts(binding(kind, json!({"concurrency":"sequential"}))).await,
			"{kind}"
		);
	}
	for kind in ["skill", "memory", "source"] {
		assert!(accepts(binding(kind, json!({}))).await, "{kind}");
		assert!(
			!accepts(binding(kind, json!({"concurrency":"sequential"}))).await,
			"{kind}"
		);
	}
	common::cleanup(f, &url, &schema).await;
}

struct ProviderTask(tokio::task::JoinHandle<()>);
impl Drop for ProviderTask {
	fn drop(&mut self) {
		self.0.abort();
	}
}

/// A Node whose research Agent binds the core file Tools, served by a local
/// model that replays scripted responses and otherwise stops.
struct Batching {
	_environment: Arc<TestEnvironment>,
	_provider: ProviderTask,
	f: Federation,
	app: TestApplication,
	token: String,
	task: Uuid,
	url: String,
	schema: String,
	root: tempfile::TempDir,
	responses: Arc<Mutex<VecDeque<Value>>>,
	requests: Arc<Mutex<Vec<Value>>>,
}

const AGENT_VERSION: &str = "1.0.1";

impl Batching {
	async fn new(environment: Arc<TestEnvironment>, parallelism: u8, slots: usize) -> Self {
		let responses = Arc::new(Mutex::new(VecDeque::<Value>::new()));
		let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
		let (scripted, captured) = (responses.clone(), requests.clone());
		let provider = Router::new().route(
			"/v1/chat/completions",
			post(move |Json(body): Json<Value>| {
				captured.lock().unwrap().push(body);
				let message = scripted.lock().unwrap().pop_front().unwrap_or_else(
					|| json!({"role":"assistant","content":"Done"}),
				);
				let finish = if message.get("tool_calls").is_some() {
					"tool_calls"
				} else {
					"stop"
				};
				async move {
					Json(json!({"choices":[{"index":0,"finish_reason":finish,"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
				}
			}),
		);
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let endpoint = format!("http://{}", listener.local_addr().unwrap());
		let provider = ProviderTask(tokio::spawn(async move {
			axum::serve(listener, provider).await.unwrap()
		}));
		let (mut f, url, schema) = common::setup(&environment).await;
		let root = tempfile::Builder::new()
			.prefix("aidash-tool-batches-")
			.tempdir()
			.unwrap();
		let mut profile = Profile {
			admission: true,
			storage: root.path().to_owned(),
			..Profile::default()
		};
		// The batch planner budgets every result at seven times its byte bound
		// (worst-case escaping). The default 32 KiB search bound alone exceeds
		// the fixture model's 128000-token window, so lower it as an operator may.
		profile.limits.search_bytes = 4096;
		f.store.capabilities = Runtime::new(profile).unwrap();
		f.store.tool_slots = ToolSlots::new(slots);
		let app = common::application(f.clone()).await;
		let (mut policy, token, task) = common::bootstrap_with_area(&f, &app, &endpoint).await;
		let mut agent = f.registry.get("research", "1.0.0").await.unwrap();
		agent.version = AGENT_VERSION.into();
		agent.binding_normalization = None;
		// A sequential Agent omits the field, as definitions written before it did.
		if parallelism != 1 {
			agent.config["tool_parallelism"] = json!(parallelism);
		}
		agent.config["max_steps"] = json!(1000);
		let operator = f.config.api_token.clone();
		let (status, body) = request(&app, &operator, "POST", "/api/registry", json!(agent)).await;
		assert_eq!(status, 200, "{body}");
		let (status, body) = request(
			&app,
			&operator,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"research","version":AGENT_VERSION},"expected_revision":0,"enabled":true}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		policy["subjects"][aidash_server::domain::qualified_agent(
			&f.config.node_id,
			"research",
			AGENT_VERSION,
		)] = json!({"kind":"agent"});
		let (status, body) = request(
			&app,
			&operator,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		Self {
			_environment: environment,
			_provider: provider,
			f,
			app,
			token,
			task,
			url,
			schema,
			root,
			responses,
			requests,
		}
	}
	async fn close(self) {
		common::cleanup(self.f, &self.url, &self.schema).await;
		drop(self.root);
	}
	/// Admit `task` to the batching Agent and return its Run.
	async fn delegate(&self, task: Uuid) -> Run {
		let (status, body) = request(
			&self.app,
			&self.token,
			"POST",
			&format!("/api/tasks/{task}/delegate"),
			json!({"node_id":self.f.config.node_id,"agent":{"id":"research","version":AGENT_VERSION}}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		self.f
			.store
			.runs()
			.await
			.unwrap()
			.into_iter()
			.find(|run| run.task_id == task)
			.unwrap()
	}
	/// A task in a fresh Workspace, so earlier Runs never grow its context.
	async fn new_task(&self) -> Uuid {
		let (status, workspace) = request(
			&self.app,
			&self.token,
			"POST",
			"/api/workspaces",
			json!({"title":"Batch","goal":"Read files in one response"}),
		)
		.await;
		assert_eq!(status, 200, "{workspace}");
		let (status, task) = request(
			&self.app,
			&self.token,
			"POST",
			&format!(
				"/api/workspaces/{}/tasks",
				workspace["id"].as_str().unwrap()
			),
			json!({"title":"Batch","description":"Read files in one response"}),
		)
		.await;
		assert_eq!(status, 200, "{task}");
		serde_json::from_value(task["id"].clone()).unwrap()
	}
	/// Materialize a text file into the Run's Working Area.
	async fn file(&self, run: &Run, path: &str, text: &str) -> Value {
		let (status, message) = request(
			&self.app,
			&self.token,
			"POST",
			&format!("/api/workspaces/{}/thread-messages", run.workspace_id),
			json!({"content":text,"idempotency_key":Uuid::new_v4()}),
		)
		.await;
		assert_eq!(status, 200, "{message}");
		let (status, area) = request(
			&self.app,
			&self.token,
			"GET",
			&format!("/api/runs/{}/working-area", run.id),
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{area}");
		let (status, created) = request(
			&self.app,
			&self.token,
			"POST",
			&format!("/api/runs/{}/files/materialize", run.id),
			json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"path":path,"source":{"kind":"message","message_id":message["message"]["id"]}}),
		)
		.await;
		assert_eq!(status, 200, "{created}");
		created["file"].clone()
	}
	fn script(&self, calls: &[(&str, &str, Value)]) {
		let tool_calls = calls
			.iter()
			.map(|(id, name, arguments)| {
				json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}})
			})
			.collect::<Vec<_>>();
		self.responses
			.lock()
			.unwrap()
			.push_back(json!({"role":"assistant","content":null,"tool_calls":tool_calls}));
	}
}

/// One worker step of a Run, observed from the persisted state before it.
struct Step {
	before: Run,
	after: Run,
	elapsed: Duration,
}

/// Drive the Run to completion with a real worker, recording every step.
async fn complete(f: &Federation, run: Uuid) -> Vec<Step> {
	let worker = Harness {
		federation: f.clone(),
	};
	let mut steps = Vec::new();
	for _ in 0..64 {
		let before = f.store.run(run).await.unwrap();
		if before.phase() == RunPhase::Completed {
			return steps;
		}
		let started = Instant::now();
		assert!(worker.worker_once().await.unwrap(), "{:?}", before.state);
		let elapsed = started.elapsed();
		let after = f.store.run(run).await.unwrap();
		assert!(
			!matches!(
				after.phase(),
				RunPhase::Failed | RunPhase::Cancelled | RunPhase::Waiting
			),
			"run stopped: phase={} error={:?}",
			after.phase(),
			after.error
		);
		steps.push(Step {
			before,
			after,
			elapsed,
		});
	}
	panic!("run {run} did not complete");
}

fn tool_events(run: &Run) -> Vec<(String, String, Value)> {
	run.context
		.history
		.iter()
		.filter_map(|event| match event {
			ContextEvent::Tool { call, result } => {
				Some((call.id.clone(), call.name.clone(), result.clone()))
			}
			_ => None,
		})
		.collect()
}

/// Journal events of one Run, in commit order.
async fn tool_journal(f: &Federation, run: Uuid) -> Vec<(String, String)> {
	f.store
		.events(0, None, 10_000)
		.await
		.unwrap()
		.into_iter()
		.filter(|event| {
			event.kind.starts_with("tool.") && event.data["run_id"] == json!(run.to_string())
		})
		.map(|event| {
			(
				event.kind,
				event.data["idempotency_key"].as_str().unwrap().to_owned(),
			)
		})
		.collect()
}

/// One raw invocation journal row of a Run.
#[derive(Debug)]
struct Journaled {
	idempotency_key: String,
	tool: String,
	status: String,
	replay_safe: bool,
	input: Value,
	result: Option<Value>,
}

async fn invocations(f: &Federation, run: Uuid) -> Vec<Journaled> {
	sqlx::query_as::<_, (String, String, String, bool, Value, Option<Value>)>(
		"SELECT idempotency_key, tool, status, replay_safe, input, result FROM invocations WHERE run_id = $1 ORDER BY idempotency_key",
	)
	.bind(run)
	.fetch_all(f.store.pool.driver())
	.await
	.unwrap()
	.into_iter()
	.map(
		|(idempotency_key, tool, status, replay_safe, input, result)| Journaled {
			idempotency_key,
			tool,
			status,
			replay_safe,
			input,
			result,
		},
	)
	.collect()
}

#[rstest::rstest]
#[tokio::test]
async fn model_response_reads_run_as_one_batch_and_are_adopted_in_call_order(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	// Arrange: Agent and Node ceilings of two, one text file in the Working Area.
	let b = Batching::new(environment, 2, 2).await;
	let run = b.delegate(b.task).await;
	let file = b
		.file(&run, "notes/東京.txt", &"東京の資料\n".repeat(4))
		.await;
	b.script(&[
		(
			"provider-search",
			"file_search",
			json!({"query":"東京","mode":"literal","scope":"working"}),
		),
		(
			"provider-read",
			"file_read",
			json!({"file_id":file["file_id"],"representation":"text","max_bytes":64}),
		),
	]);
	// Act: a real worker infers, executes the batch and finishes the Run.
	let steps = complete(&b.f, run.id).await;
	// Assert: one step admitted, executed and adopted both calls.
	let batch = steps
		.iter()
		.find(|step| step.before.phase() == RunPhase::ToolCall)
		.expect("the model response entered ToolCall");
	let before = batch.before.state.tool().unwrap();
	assert_eq!(before.cursor, 0);
	assert_eq!(before.response.tool_calls.len(), 2);
	let after = batch.after.state.tool().unwrap();
	assert_eq!(after.cursor, 2, "both calls must be adopted by one step");
	assert_eq!(after.batch_end, None);
	let epoch = before.response_epoch;
	let keys = [0, 1].map(|index| format!("{}:{epoch}:{index}", run.id));
	let journal = invocations(&b.f, run.id).await;
	assert_eq!(
		journal
			.iter()
			.map(|row| (
				row.idempotency_key.as_str(),
				row.tool.as_str(),
				row.status.as_str(),
				row.replay_safe
			))
			.collect::<Vec<_>>(),
		vec![
			(keys[0].as_str(), "file_search", "COMPLETED", true),
			(keys[1].as_str(), "file_read", "COMPLETED", true),
		]
	);
	// Admission journals both calls before either completes.
	let events = tool_journal(&b.f, run.id).await;
	assert_eq!(events.len(), 4, "{events:?}");
	assert!(events[..2].iter().all(|(kind, _)| kind == "tool.started"));
	assert_eq!(
		events[..2].iter().map(|(_, key)| key).collect::<Vec<_>>(),
		vec![&keys[0], &keys[1]]
	);
	assert!(events[2..].iter().all(|(kind, _)| kind == "tool.completed"));
	// Observations keep the model's order and provider call IDs; each result
	// carries its own read receipt rather than a shared batch snapshot.
	let finished = b.f.store.run(run.id).await.unwrap();
	assert_eq!(finished.phase(), RunPhase::Completed);
	let observed = tool_events(&finished);
	assert_eq!(
		observed
			.iter()
			.map(|(id, name, _)| (id.as_str(), name.as_str()))
			.collect::<Vec<_>>(),
		vec![
			("provider-search", "file_search"),
			("provider-read", "file_read")
		]
	);
	let (_, _, search) = &observed[0];
	let (_, _, read) = &observed[1];
	assert_eq!(search["matches"][0]["file_id"], file["file_id"], "{search}");
	assert!(
		read["content"].as_str().unwrap().starts_with("東京"),
		"{read}"
	);
	for (index, result) in [search, read].into_iter().enumerate() {
		assert!(result.get("error").is_none(), "{result}");
		assert_eq!(
			result["area_id"],
			json!(run_area(&b, &run).await),
			"{result}"
		);
		assert!(result["revision"].is_i64(), "{result}");
		assert!(result["generation"].is_i64(), "{result}");
		assert_eq!(
			journal[index].result.as_ref(),
			Some(result),
			"the journal holds the adopted result"
		);
	}
	// The Run then proceeds normally: the next inference sees both results.
	let requests = b.requests.lock().unwrap().clone();
	assert_eq!(requests.len(), 2, "{requests:?}");
	let next = requests[1].to_string();
	let (search_at, read_at) = (
		next.find("provider-search")
			.expect("search result reaches inference"),
		next.find("provider-read")
			.expect("read result reaches inference"),
	);
	assert!(search_at < read_at);
	b.close().await;
}

async fn run_area(b: &Batching, run: &Run) -> Value {
	let (status, area) = request(
		&b.app,
		&b.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	area["id"].clone()
}

/// A leased Run positioned at a two-call model response.
async fn leased_tool_run(b: &Batching) -> (Run, Uuid) {
	let run = b.delegate(b.task).await;
	let worker = Uuid::new_v4();
	let mut leased = b.f.store.lease_run(worker, 30).await.unwrap().unwrap();
	assert_eq!(leased.id, run.id);
	leased.state = RunState::ToolCall(Box::new(common::tool_call(
		json!({"included_input_seq":leased.observed_input_seq}),
	)));
	leased.state.tool_mut().unwrap().batch_end = Some(2);
	(leased, worker)
}

fn batch_calls(run: &Run) -> (Vec<String>, Vec<Value>) {
	let keys = (0..2)
		.map(|index| format!("{}:7:{index}", run.id))
		.collect();
	let inputs = vec![
		json!({"query":"東京","mode":"literal","scope":"working"}),
		json!({"file_id":Uuid::new_v4(),"representation":"text"}),
	];
	(keys, inputs)
}

async fn admit(
	store: &Store,
	run: &Run,
	worker: Uuid,
	keys: &[String],
	inputs: &[Value],
) -> aidash_server::Result<Vec<Invocation>> {
	let names = ["file_search", "file_read"];
	let calls = keys
		.iter()
		.zip(names)
		.zip(inputs)
		.map(|((key, name), input)| (key.as_str(), name, input))
		.collect::<Vec<_>>();
	store.invocation_start_batch(run, worker, &calls).await
}

#[rstest::rstest]
#[tokio::test]
async fn batch_admission_journals_every_call_with_the_run_and_replays_unchanged(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	// Arrange
	let b = Batching::new(environment, 1, 1).await;
	let (run, worker) = leased_tool_run(&b).await;
	let (keys, inputs) = batch_calls(&run);
	// Act
	let admitted = admit(&b.f.store, &run, worker, &keys, &inputs)
		.await
		.unwrap();
	// Assert: one STARTED replay-safe row per call and the persisted batch end.
	assert_eq!(
		admitted
			.iter()
			.map(|row| (
				row.idempotency_key.clone(),
				row.status.clone(),
				row.replay_safe
			))
			.collect::<Vec<_>>(),
		keys.iter()
			.map(|key| (key.clone(), "STARTED".to_owned(), true))
			.collect::<Vec<_>>()
	);
	let journal = invocations(&b.f, run.id).await;
	assert_eq!(journal.len(), 2);
	assert_eq!(
		journal.iter().map(|row| &row.input).collect::<Vec<_>>(),
		inputs.iter().collect::<Vec<_>>()
	);
	let stored = b.f.store.run(run.id).await.unwrap();
	assert_eq!(stored.phase(), RunPhase::ToolCall);
	assert_eq!(stored.state.tool().unwrap().batch_end, Some(2));
	assert_eq!(stored.revision, run.revision + 1);
	// Act: complete the first call, then admit the same batch again (restart).
	b.f.store
		.invocation_finish(&run, worker, &keys[0], &json!({"done":0}))
		.await
		.unwrap();
	let readmitted = admit(&b.f.store, &run, worker, &keys, &inputs)
		.await
		.unwrap();
	// Assert: existing rows are returned unchanged, never duplicated or restarted.
	assert_eq!(readmitted[0].status, "COMPLETED");
	assert_eq!(readmitted[0].result, Some(json!({"done":0})));
	assert_eq!(readmitted[1].status, "STARTED");
	assert!(readmitted.iter().all(|row| row.replay_safe));
	assert_eq!(invocations(&b.f, run.id).await.len(), 2);
	assert_eq!(
		tool_journal(&b.f, run.id).await,
		vec![
			("tool.started".to_owned(), keys[0].clone()),
			("tool.started".to_owned(), keys[1].clone()),
			("tool.completed".to_owned(), keys[0].clone()),
		]
	);
	b.close().await;
}

#[derive(Clone, Copy, Debug)]
enum Rejection {
	NewerInput,
	OtherWorker,
	ExpiredLease,
	ConflictingCall,
}

#[rstest::rstest]
#[case::newer_input(Rejection::NewerInput)]
#[case::lost_lease(Rejection::OtherWorker)]
#[case::expired_lease(Rejection::ExpiredLease)]
#[case::conflicting_later_call(Rejection::ConflictingCall)]
#[tokio::test]
async fn rejected_batch_admission_writes_nothing(
	#[case] rejection: Rejection,
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	// Arrange
	let b = Batching::new(environment, 1, 1).await;
	let (run, mut worker) = leased_tool_run(&b).await;
	let (keys, inputs) = batch_calls(&run);
	let driver = b.f.store.pool.driver();
	let mut existing = 0;
	match rejection {
		Rejection::NewerInput => {
			sqlx::query(
				"INSERT INTO run_inputs (run_id, sender, content, idempotency_key) VALUES ($1, 'human', 'correction', $2)",
			)
			.bind(run.id)
			.bind(format!("human:{}:{}", run.id, Uuid::new_v4()))
			.execute(driver)
			.await
			.unwrap();
		}
		Rejection::OtherWorker => worker = Uuid::new_v4(),
		Rejection::ExpiredLease => {
			sqlx::query(
				"UPDATE runs SET lease_until = CURRENT_TIMESTAMP - INTERVAL '1 second' WHERE id = $1",
			)
			.bind(run.id)
			.execute(driver)
			.await
			.unwrap();
		}
		Rejection::ConflictingCall => {
			// The later key is already journaled for a different input.
			let mut sequential = run.clone();
			sequential.state.tool_mut().unwrap().batch_end = None;
			b.f.store
				.invocation_start(
					&sequential,
					worker,
					&keys[1],
					"file_read",
					&json!({"file_id":Uuid::new_v4(),"representation":"text"}),
					true,
				)
				.await
				.unwrap();
			existing = 1;
		}
	}
	let stored = b.f.store.run(run.id).await.unwrap();
	let events = tool_journal(&b.f, run.id).await;
	// Act
	let result = admit(&b.f.store, &run, worker, &keys, &inputs).await;
	// Assert: no row, event or Run update survives a rejected admission.
	assert!(
		match rejection {
			// A newer Run input makes the response stale before any effect.
			Rejection::NewerInput => matches!(result, Err(Error::StaleInference)),
			_ => matches!(result, Err(Error::Conflict(_))),
		},
		"{rejection:?}: {result:?}"
	);
	let journal = invocations(&b.f, run.id).await;
	assert_eq!(journal.len(), existing, "{journal:?}");
	assert!(journal.iter().all(|row| row.idempotency_key != keys[0]));
	assert_eq!(tool_journal(&b.f, run.id).await, events);
	let unchanged = b.f.store.run(run.id).await.unwrap();
	assert_eq!(unchanged.revision, stored.revision);
	assert_eq!(
		unchanged
			.state
			.tool()
			.ok()
			.and_then(|state| state.batch_end),
		None
	);
	b.close().await;
}

fn percentile(samples: &mut [Duration], percentile: usize) -> Duration {
	samples.sort();
	samples[((samples.len() - 1) * percentile) / 100]
}

fn report(label: &str, samples: &mut [Duration]) {
	let (p50, p95) = (percentile(samples, 50), percentile(samples, 95));
	println!(
		"{label:<44} n={:<3} p50={:>8.2}ms p95={:>8.2}ms",
		samples.len(),
		p50.as_secs_f64() * 1e3,
		p95.as_secs_f64() * 1e3
	);
}

/// Wall time of the worker steps that executed one model response's calls.
fn tool_phase(steps: &[Step]) -> (Duration, usize) {
	let tool_steps = steps
		.iter()
		.filter(|step| step.before.phase() == RunPhase::ToolCall)
		.filter(|step| {
			let before = step.before.state.tool().unwrap();
			before.cursor < before.response.tool_calls.len()
		})
		.collect::<Vec<_>>();
	(
		tool_steps.iter().map(|step| step.elapsed).sum(),
		tool_steps.len(),
	)
}

/// Prints latency only; it asserts nothing about speedup.
#[rstest::rstest]
#[tokio::test]
#[ignore = "benchmark: run with -- --ignored --nocapture"]
async fn tool_batch_latency_benchmark(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	const ITERATIONS: usize = 10;
	const READS: usize = 4;
	let b = Batching::new(environment, 4, 4).await;
	let first = b.delegate(b.task).await;
	complete(&b.f, first.id).await;
	let mut sequential = b.f.clone();
	sequential.store.tool_slots = ToolSlots::new(1);
	let text = "東京の資料 with a few searchable words\n".repeat(16);
	for (workload, writes) in [("reads", false), ("interleaved read/write", true)] {
		for (label, federation) in [("parallelism 1", &sequential), ("parallelism 4", &b.f)] {
			let mut tool_phases = Vec::new();
			let mut whole = Vec::new();
			let mut step_counts = Vec::new();
			for iteration in 0..ITERATIONS {
				let task = b.new_task().await;
				let run = b.delegate(task).await;
				let mut files = Vec::new();
				for index in 0..READS {
					files.push(
						b.file(&run, &format!("bench/{iteration}-{index}.txt"), &text)
							.await,
					);
				}
				let mut calls = Vec::new();
				for (index, file) in files.iter().enumerate() {
					if writes && index == READS / 2 {
						calls.push((
							"write".to_owned(),
							"workspace_message",
							json!({"content":format!("progress {iteration}")}),
						));
					}
					calls.push((
						format!("read-{index}"),
						"file_read",
						json!({"file_id":file["file_id"],"representation":"text","max_bytes":512}),
					));
				}
				b.script(
					&calls
						.iter()
						.map(|(id, name, input)| (id.as_str(), *name, input.clone()))
						.collect::<Vec<_>>(),
				);
				let started = Instant::now();
				let steps = complete(federation, run.id).await;
				whole.push(started.elapsed());
				let (phase, count) = tool_phase(&steps);
				tool_phases.push(phase);
				step_counts.push(count);
				let observed = tool_events(&b.f.store.run(run.id).await.unwrap());
				assert_eq!(observed.len(), calls.len());
			}
			let workload = format!("{workload} ({label})");
			println!(
				"{workload}: tool steps per response {:?}",
				step_counts.iter().max()
			);
			report(&format!("{workload} tool phase"), &mut tool_phases);
			report(&format!("{workload} whole run"), &mut whole);
		}
	}
	// Admission alone: one batch transaction versus one per call.
	let mut batched = Vec::new();
	let mut separate = Vec::new();
	for iteration in 0..ITERATIONS {
		let task = b.new_task().await;
		let run = b.delegate(task).await;
		let worker = Uuid::new_v4();
		let mut leased = b.f.store.lease_run(worker, 30).await.unwrap().unwrap();
		assert_eq!(leased.id, run.id);
		leased.state = RunState::ToolCall(Box::new(common::tool_call(
			json!({"included_input_seq":leased.observed_input_seq}),
		)));
		let input = json!({"file_id":Uuid::new_v4(),"representation":"text"});
		let keys = (0..READS)
			.map(|index| format!("{}:{iteration}:{index}", run.id))
			.collect::<Vec<_>>();
		leased.state.tool_mut().unwrap().batch_end = Some(READS);
		let calls = keys
			.iter()
			.map(|key| (key.as_str(), "file_read", &input))
			.collect::<Vec<_>>();
		let started = Instant::now();
		b.f.store
			.invocation_start_batch(&leased, worker, &calls)
			.await
			.unwrap();
		batched.push(started.elapsed());
		leased.state.tool_mut().unwrap().batch_end = None;
		let started = Instant::now();
		for key in &keys {
			b.f.store
				.invocation_start(
					&leased,
					worker,
					&format!("{key}:sequential"),
					"file_read",
					&input,
					true,
				)
				.await
				.unwrap();
		}
		separate.push(started.elapsed());
		sqlx::query(
			"UPDATE runs SET phase = 'COMPLETED', lease_owner = NULL, lease_until = NULL WHERE id = $1",
		)
		.bind(run.id)
		.execute(b.f.store.pool.driver())
		.await
		.unwrap();
	}
	report(
		&format!("admission of {READS} calls (one batch tx)"),
		&mut batched,
	);
	report(
		&format!("admission of {READS} calls (one tx per call)"),
		&mut separate,
	);
	b.close().await;
}
