use super::*;
use crate::ports::registry::workbench::sandbox::{
	SandboxScope,
	dispatch::{PreparedRealRequest, RealDispatchRepository, RealDispatchScope},
	execution::ExecutionScope,
};
use aidash_domain::{
	provider::{ModelResponse, ToolCall},
	registry::{
		Entry,
		workbench::{
			profile::{RealToolRule, TestProfile},
			sandbox::{Fixture, FixtureStatus, TestSession},
		},
	},
};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use rstest::rstest;
use sha2::Digest;
use std::{
	collections::{BTreeMap, VecDeque},
	sync::Mutex,
};
struct State {
	active: bool,
	commits: usize,
	begun: usize,
	reads: usize,
	session: TestSession,
	finished: Option<TestOutcome>,
	order: Vec<&'static str>,
}
struct Repository {
	entry: Entry,
	state: Mutex<State>,
	failure: Option<(usize, &'static str)>,
	stop_after_progress: bool,
}
impl Repository {
	fn new() -> Self {
		Self {
			entry: crate::test_support::agent("agent"),
			state: Mutex::new(State {
				active: false,
				commits: 0,
				begun: 0,
				reads: 0,
				session: session(),
				finished: None,
				order: vec![],
			}),
			failure: None,
			stop_after_progress: false,
		}
	}
	fn failing(&self, name: &str) -> bool {
		self.failure
			.is_some_and(|(phase, kind)| phase == self.state.lock().unwrap().begun && kind == name)
	}
}
struct Scope<'a>(&'a Repository);
impl Drop for Scope<'_> {
	fn drop(&mut self) {
		self.0.state.lock().unwrap().active = false;
	}
}
fn draft(revision: i64) -> Draft {
	Draft {
		id: Uuid::from_u128(1),
		tenant: "tenant".into(),
		owner: "owner".into(),
		revision,
		entry: json!({}),
		documents: json!([]),
		release_notes: String::new(),
		source_id: None,
		source_version: None,
		archived: false,
		updated_at: Utc.timestamp_opt(100, 0).unwrap(),
	}
}
fn session() -> TestSession {
	TestSession {
		id: Uuid::from_u128(9),
		draft_id: Uuid::from_u128(1),
		tenant: "tenant".into(),
		revision: 7,
		status: "running".into(),
		scenario: json!({"binding_snapshot":crate::test_support::resolve("aidash://local", &crate::test_support::agent("agent"), false, vec![])}),
		conversation: Some(json!([{"role":"user","content":"original"}])),
		tool_calls: Some(json!([])),
		usage: json!({"input_tokens":0,"output_tokens":0,"usage_complete":true}),
		error: None,
		created_at: Utc.timestamp_opt(100, 0).unwrap(),
		updated_at: Utc.timestamp_opt(100, 0).unwrap(),
		expires_at: Utc.timestamp_opt(200, 0).unwrap(),
		expired_at: None,
	}
}
#[async_trait]
impl RealDispatchRepository for Repository {
	async fn begin_real(&self) -> Result<Box<dyn RealDispatchScope + '_>> {
		panic!("simulated fixtures must not dispatch real tools")
	}
}
#[async_trait]
impl ExecutionRepository for Repository {
	async fn begin_execution(&self) -> Result<Box<dyn ExecutionScope + '_>> {
		let mut s = self.state.lock().unwrap();
		assert!(!s.active);
		s.active = true;
		s.begun += 1;
		s.order.push("begin");
		Ok(Box::new(Scope(self)))
	}
	async fn read_session(&self, id: Uuid) -> Result<TestSession> {
		assert_eq!(id, session().id);
		let mut s = self.state.lock().unwrap();
		assert!(!s.active);
		s.reads += 1;
		s.order.push("snapshot");
		Ok(s.session.clone())
	}
	async fn progress(
		&self,
		id: Uuid,
		conversation: Value,
		calls: Value,
		usage: Value,
	) -> Result<()> {
		assert_eq!(id, session().id);
		let mut s = self.state.lock().unwrap();
		assert!(!s.active);
		s.order.push("progress");
		if s.session.status == "running" {
			s.session.conversation = Some(conversation);
			s.session.tool_calls = Some(calls);
			s.session.usage = usage;
			if self.stop_after_progress {
				s.session.status = "stopped".into();
			}
		}
		Ok(())
	}
	async fn finish(&self, id: Uuid, outcome: &TestOutcome) -> Result<()> {
		assert_eq!(id, session().id);
		let mut s = self.state.lock().unwrap();
		assert!(!s.active);
		s.order.push("finish");
		if s.session.status == "running" {
			s.finished = Some(outcome.clone());
			s.session.status = outcome.status.clone();
		}
		Ok(())
	}
}
#[async_trait]
impl SandboxScope for Scope<'_> {
	async fn draft(&mut self, id: Uuid, lock: bool) -> Result<Draft> {
		assert_eq!(id, draft(7).id);
		assert!(!lock);
		self.0.state.lock().unwrap().order.push("current_draft");
		Ok(draft(7))
	}
	async fn authorize_draft(&mut self, current: &Draft, action: &str, shares: bool) -> Result<()> {
		assert_eq!(current.revision, 7);
		assert_eq!(action, "agent_draft.test");
		assert!(shares);
		self.0.state.lock().unwrap().order.push("current_authority");
		if self.0.failing("authority") {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn limits(&mut self, _: &str) -> Result<TestLimits> {
		panic!("admitted limits must remain pinned")
	}
	async fn save_limits(&mut self, _: &TestLimits) -> Result<()> {
		panic!("execution must not alter limits")
	}
	async fn session(&mut self, _: Uuid, _: bool) -> Result<TestSession> {
		panic!("dispatch session lease is unused for fixtures")
	}
	async fn sessions(&mut self, _: Uuid) -> Result<Vec<TestSession>> {
		panic!("execution must not page sessions")
	}
	async fn stop(&mut self, _: Uuid) -> Result<TestSession> {
		panic!("execution must not overwrite stop")
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		let mut s = self.0.state.lock().unwrap();
		s.commits += 1;
		s.order.push("commit");
		Ok(())
	}
}
#[async_trait]
impl RealDispatchScope for Scope<'_> {
	async fn lock_identity(&mut self) -> Result<()> {
		self.0.state.lock().unwrap().order.push("identity");
		Ok(())
	}
	async fn profile(&mut self, tenant: &str, id: &str) -> Result<TestProfile> {
		assert_eq!((tenant, id), ("tenant", "profile"));
		self.0.state.lock().unwrap().order.push("profile");
		Ok(TestProfile {
			tenant: tenant.into(),
			id: id.into(),
			revision: if self.0.failing("profile_revision") {
				8
			} else {
				7
			},
			enabled: !self.0.failing("profile_disabled"),
			rules: if self.0.failing("profile_rules") {
				json!(["changed"])
			} else {
				json!([])
			},
			updated_at: Utc.timestamp_opt(100, 0).unwrap(),
		})
	}
	async fn effective(&mut self, _: &EntityRef) -> Result<Entry> {
		panic!("dispatch only")
	}
	async fn write_calls(&mut self, _: Uuid, _: Value, _: bool) -> Result<()> {
		panic!("dispatch only")
	}
	async fn rollback(self: Box<Self>) -> Result<()> {
		panic!("dispatch only")
	}
}
#[async_trait]
impl ExecutionScope for Scope<'_> {
	async fn bindings(
		&mut self,
		_: &Draft,
		entry: &Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		Ok(crate::test_support::resolve(
			"aidash://local",
			entry,
			false,
			vec![],
		))
	}

	async fn validate_content(&mut self, pinned: &Draft) -> Result<Entry> {
		assert_eq!(pinned.revision, 7);
		self.0
			.state
			.lock()
			.unwrap()
			.order
			.push("pinned_dependencies");
		if self.0.failing("dependencies") {
			Err(Error::Forbidden)
		} else {
			Ok(self.0.entry.clone())
		}
	}
}
impl Credentials for Repository {
	fn resolve(&self, name: &str) -> Result<String> {
		assert_eq!(name, "MODEL_TEST");
		Ok(if self.failing("credential") {
			"rotated-fixture"
		} else {
			"fixture"
		}
		.into())
	}
}
impl ProfileConfiguration for Repository {
	fn require_secret(&self, _: &str) -> Result<()> {
		panic!("fixtures must not resolve tool credentials")
	}
}
impl RealToolTransport for Repository {
	fn prepare(
		&self,
		_: Uuid,
		_: &RealToolRule,
		_: &ToolCall,
	) -> Result<Box<dyn PreparedRealRequest>> {
		panic!("fixtures must never use transport")
	}
}
struct Model {
	repository: Arc<Repository>,
	responses: Mutex<VecDeque<ModelResponse>>,
	requests: Mutex<Vec<ModelRequest>>,
	failure: bool,
	pause: bool,
}
#[async_trait]
impl ModelProvider for Model {
	async fn infer(&self, request: ModelRequest) -> Result<ModelResponse> {
		{
			let mut s = self.repository.state.lock().unwrap();
			assert!(!s.active);
			assert_eq!(s.begun, s.commits);
			s.order.push("infer");
		}
		self.requests.lock().unwrap().push(request);
		if self.pause {
			return std::future::pending().await;
		}
		if self.failure {
			return Err(Error::External("inference failed".into()));
		}
		Ok(self
			.responses
			.lock()
			.unwrap()
			.pop_front()
			.expect("unexpected additional inference"))
	}
}
fn response(tool: bool) -> ModelResponse {
	ModelResponse {
		text: if tool { "request" } else { "done" }.into(),
		tool_calls: if tool {
			vec![ToolCall {
				id: "call".into(),
				name: "workspace_message".into(),
				arguments: json!({"content":"fixture"}),
			}]
		} else {
			vec![]
		},
		input_tokens: 3,
		output_tokens: 5,
		usage_complete: true,
	}
}
fn execution(repository: Arc<Repository>) -> Execution {
	Execution {
		repository: repository.clone(),
		credentials: repository.clone(),
		configuration: repository.clone(),
		transport: repository,
	}
}
fn model(repository: Arc<Repository>, responses: Vec<ModelResponse>) -> Arc<Model> {
	Arc::new(Model {
		repository,
		responses: Mutex::new(responses.into()),
		requests: Mutex::new(vec![]),
		failure: false,
		pause: false,
	})
}
fn job(model: Arc<Model>) -> Job {
	Job {
		input: TestInput {
			expected_revision: 7,
			message: "original".into(),
			mode: "simulated".into(),
			profile_id: None,
			continue_from: None,
			fixtures: BTreeMap::new(),
		},
		profile: None,
		tool_references: Default::default(),
		limits: TestLimits {
			tenant: "tenant".into(),
			max_input_bytes: 65536,
			max_output_tokens: 128,
			max_total_tokens: 10000,
			max_steps: 3,
			max_duration_secs: 30,
			max_concurrent: 2,
			payload_days: 3,
			incident_evidence_days: 7,
		},
		context_window: 20000,
		agent_max_steps: 3,
		model_provider: model,
		request: ModelRequest {
			instructions: "sandbox".into(),
			context: json!({"test_message":"original"}).into(),
			tools: vec![],
			max_output_tokens: 128,
			content_parts: vec![],
			cache_scope: None,
		},
		initial_conversation: vec![json!({"role":"user","content":"original"})],
		pinned_draft: draft(7),
		model_credential: None,
		exposure: None,
	}
}
#[tokio::test]
async fn explicit_fixture_is_recorded_and_reauthorized_before_each_inference() {
	// Arrange a model continuation after a fixture-only Tool call.
	let repository = Arc::new(Repository::new());
	let model = model(repository.clone(), vec![response(true), response(false)]);
	let mut job = job(model.clone());
	job.input.fixtures.insert(
		"workspace_message".into(),
		Fixture {
			status: FixtureStatus::Success,
			response: json!({"fixture":true}),
		},
	);
	// Act using no real transport or runtime executor.
	let outcome = simulate(&execution(repository.clone()), session().id, &job)
		.await
		.unwrap();
	// Assert exact usage, remaining output budget and persisted fixture evidence.
	assert_eq!(outcome.status, "completed");
	assert_eq!(outcome.error, None);
	assert_eq!(
		outcome.usage,
		json!({"input_tokens":6,"output_tokens":10,"usage_complete":true})
	);
	assert_eq!(
		outcome.tool_calls,
		json!([{"id":"call","name":"workspace_message","arguments":{"content":"fixture"},"fixture":{"status":"success","response":{"fixture":true}},"outcome":"simulated"}])
	);
	let requests = model.requests.lock().unwrap();
	assert_eq!(requests.len(), 2);
	assert_eq!(requests[1].max_output_tokens, 123);
	let aidash_domain::provider::ModelContext::Legacy(context) = &requests[1].context else {
		panic!("Legacy request context");
	};
	assert_eq!(context["conversation"].as_array().unwrap().len(), 3);
	let state = repository.state.lock().unwrap();
	assert_eq!(state.commits, 2);
	assert!(!state.active);
	assert_eq!(
		state.order,
		[
			"snapshot",
			"begin",
			"identity",
			"current_draft",
			"current_authority",
			"pinned_dependencies",
			"commit",
			"infer",
			"progress",
			"snapshot",
			"begin",
			"identity",
			"current_draft",
			"current_authority",
			"pinned_dependencies",
			"commit",
			"infer"
		]
	);
}
#[tokio::test]
async fn deferred_load_results_reshape_the_next_request() {
	// Arrange: a deferred Agent whose default workspace tools are Discoverable.
	let mut repository = Repository::new();
	repository.entry.config["exposure"] = json!({"version":"deferred@1"});
	let snapshot = crate::test_support::resolve("aidash://local", &repository.entry, false, vec![]);
	repository.state.get_mut().unwrap().session.scenario = json!({"binding_snapshot":snapshot});
	let repository = Arc::new(repository);
	let digest = snapshot
		.bindings
		.iter()
		.find(|binding| binding.alias.as_deref() == Some("workspace_observe"))
		.unwrap()
		.digest
		.clone();
	let call = |id: &str, name: &str, arguments: Value| ToolCall {
		id: id.into(),
		name: name.into(),
		arguments,
	};
	let respond = |tool_calls| ModelResponse {
		text: "step".into(),
		tool_calls,
		input_tokens: 3,
		output_tokens: 5,
		usage_complete: true,
	};
	let model = model(
		repository.clone(),
		vec![
			respond(vec![
				call(
					"load",
					"capability_load",
					json!({"alias":"workspace_observe","digest":digest}),
				),
				call("early", "workspace_observe", json!({})),
			]),
			respond(vec![]),
		],
	);
	let mut job = job(model.clone());
	job.limits.max_total_tokens = 100_000;
	job.context_window = 100_000;
	let state = ExposureState::default();
	let (text, tools) = deferred::Deferred::new(&snapshot)
		.unwrap()
		.unwrap()
		.request(&snapshot, &state)
		.unwrap();
	job.request.instructions = format!("sandbox{text}");
	job.request.tools = tools;
	job.exposure = Some(SessionExposure {
		prefix: "sandbox".into(),
		suffix: String::new(),
		state,
	});
	// Act
	let outcome = simulate(&execution(repository), session().id, &job)
		.await
		.unwrap();
	// Assert: Load is evaluated natively and exposes the tool from the next request.
	assert_eq!(outcome.status, "completed");
	assert_eq!(outcome.error, None);
	assert_eq!(outcome.tool_calls[0]["outcome"], deferred::EVALUATED);
	assert_eq!(outcome.tool_calls[0]["result"]["status"], "loaded");
	assert_eq!(outcome.tool_calls[1]["outcome"], deferred::EVALUATED);
	assert_eq!(
		outcome.tool_calls[1]["result"]["error"],
		"capability workspace_observe was loaded in this response; call it after the next model request"
	);
	let requests = model.requests.lock().unwrap();
	let exposes = |request: &ModelRequest| {
		request
			.tools
			.iter()
			.any(|tool| tool.name == "workspace_observe")
	};
	assert!(!exposes(&requests[0]));
	assert!(exposes(&requests[1]));
	assert!(
		requests[0]
			.instructions
			.contains("workspace_observe [tool]")
	);
	assert!(
		!requests[1]
			.instructions
			.contains("workspace_observe [tool]")
	);
	assert!(requests[1].instructions.starts_with("sandbox"));
	// A continued session resumes the Exposure set it ended with.
	let replayed = deferred::replayed(outcome.conversation.as_array().unwrap()).unwrap();
	assert_eq!(replayed.loaded.len(), 1);
	assert_eq!(replayed.loaded[0].alias, "workspace_observe");
}
#[tokio::test]
async fn absent_fixture_blocks_without_claiming_a_tool_result() {
	let repository = Arc::new(Repository::new());
	let job = job(model(repository.clone(), vec![response(true)]));
	let outcome = simulate(&execution(repository), session().id, &job)
		.await
		.unwrap();
	assert_eq!(outcome.status, "blocked");
	assert_eq!(
		outcome.error.as_deref(),
		Some("a tool call has no explicit simulated fixture")
	);
	assert_eq!(outcome.tool_calls[0]["outcome"], "missing_fixture");
	assert_eq!(outcome.tool_calls[0]["fixture"], Value::Null);
}
#[rstest]
#[case("authority")]
#[case("dependencies")]
#[tokio::test]
async fn fresh_revocation_after_progress_prevents_a_second_inference(
	#[case] failure: &'static str,
) {
	let mut repository = Repository::new();
	repository.failure = Some((2, failure));
	let repository = Arc::new(repository);
	let model = model(repository.clone(), vec![response(true), response(false)]);
	let mut job = job(model.clone());
	job.input.fixtures.insert(
		"workspace_message".into(),
		Fixture {
			status: FixtureStatus::Denied,
			response: json!({"denied":true}),
		},
	);
	let error = simulate(&execution(repository.clone()), session().id, &job)
		.await
		.unwrap_err();
	assert!(matches!(error, Error::Forbidden));
	settle(
		repository.as_ref(),
		session().id,
		&job.input.message,
		Err(Failure::Execution(error)),
	)
	.await
	.unwrap();
	assert_eq!(model.requests.lock().unwrap().len(), 1);
	let state = repository.state.lock().unwrap();
	assert_eq!(state.commits, 1);
	assert!(!state.active);
	let outcome = state.finished.as_ref().unwrap();
	assert_eq!(outcome.status, "failed");
	assert_eq!(outcome.conversation.as_array().unwrap().len(), 3);
	assert_eq!(outcome.tool_calls[0]["outcome"], "simulated");
}
#[rstest]
#[case("profile_revision")]
#[case("profile_disabled")]
#[case("profile_rules")]
#[tokio::test]
async fn changed_profile_prevents_inference(#[case] failure: &'static str) {
	let mut repository = Repository::new();
	repository.failure = Some((1, failure));
	let repository = Arc::new(repository);
	let model = model(repository.clone(), vec![response(false)]);
	let mut job = job(model.clone());
	job.profile = Some(ProfilePin {
		id: "profile".into(),
		revision: 7,
		tenant: "tenant".into(),
		rules: vec![],
		credential_fingerprints: BTreeMap::new(),
	});
	assert!(matches!(
		simulate(&execution(repository.clone()), session().id, &job).await,
		Err(Error::Conflict(_))
	));
	assert_eq!(model.requests.lock().unwrap().len(), 0);
	assert!(!repository.state.lock().unwrap().active);
}
#[tokio::test]
async fn rotated_model_credential_prevents_authority_and_inference() {
	let mut repository = Repository::new();
	repository.failure = Some((0, "credential"));
	let repository = Arc::new(repository);
	let model = model(repository.clone(), vec![]);
	let mut job = job(model.clone());
	job.model_credential = Some((
		"MODEL_TEST".into(),
		sha2::Sha256::digest(b"fixture").to_vec(),
	));
	assert!(matches!(
		simulate(&execution(repository.clone()), session().id, &job).await,
		Err(Error::Conflict(_))
	));
	assert_eq!(repository.state.lock().unwrap().begun, 0);
	assert_eq!(model.requests.lock().unwrap().len(), 0);
}
#[tokio::test]
async fn stopped_session_performs_no_authority_or_inference() {
	let repository = Arc::new(Repository::new());
	repository.state.lock().unwrap().session.status = "stopped".into();
	let model = model(repository.clone(), vec![]);
	let job = job(model.clone());
	let outcome = simulate(&execution(repository.clone()), session().id, &job)
		.await
		.unwrap();
	assert_eq!(outcome.status, "stopped");
	assert_eq!(outcome.error, None);
	assert_eq!(repository.state.lock().unwrap().begun, 0);
	assert_eq!(model.requests.lock().unwrap().len(), 0);
}
#[rstest]
#[case(
	"incomplete",
	"provider usage is incomplete; test token limits cannot be verified"
)]
#[case("output", "test output token limit exceeded by model response")]
#[case("total", "test total token limit exceeded by model response")]
#[tokio::test]
async fn unverified_or_excess_usage_blocks_before_disclosing_model_output(
	#[case] kind: &str,
	#[case] error: &str,
) {
	let repository = Arc::new(Repository::new());
	let mut returned = response(false);
	if kind == "incomplete" {
		returned.usage_complete = false;
	} else if kind == "output" {
		returned.output_tokens = 129;
	} else {
		returned.input_tokens = 10001;
	}
	let job = job(model(repository.clone(), vec![returned]));
	let outcome = simulate(&execution(repository), session().id, &job)
		.await
		.unwrap();
	assert_eq!(outcome.status, "blocked");
	assert_eq!(outcome.error.as_deref(), Some(error));
	assert_eq!(outcome.conversation.as_array().unwrap().len(), 1);
	assert_eq!(outcome.tool_calls, json!([]));
}
#[rstest]
#[case("bytes")]
#[case("window")]
#[case("total")]
#[tokio::test]
async fn input_budget_rejection_occurs_before_inference(#[case] budget: &str) {
	let repository = Arc::new(Repository::new());
	let model = model(repository.clone(), vec![]);
	let mut job = job(model.clone());
	match budget {
		"bytes" => job.limits.max_input_bytes = 1,
		"window" => job.context_window = 1,
		_ => job.limits.max_total_tokens = 1,
	}
	let outcome = simulate(&execution(repository), session().id, &job)
		.await
		.unwrap();
	assert_eq!(outcome.status, "blocked");
	assert_eq!(
		outcome.error.as_deref(),
		Some("test context exceeds configured input or model window limit")
	);
	assert_eq!(model.requests.lock().unwrap().len(), 0);
}
#[rstest]
#[case(false, false, "failed")]
#[case(false, true, "timed_out")]
#[case(true, false, "outcome_unknown")]
#[case(true, true, "outcome_unknown")]
#[tokio::test]
async fn completion_classifies_failure_from_durable_evidence(
	#[case] unknown: bool,
	#[case] timeout: bool,
	#[case] status: &str,
) {
	let repository = Arc::new(Repository::new());
	let job = job(model(repository.clone(), vec![]));
	let prior = json!([{"id":"call","outcome":if unknown {"outcome_unknown"} else {"real"}}]);
	repository.state.lock().unwrap().session.tool_calls = Some(prior.clone());
	let failure = if timeout {
		Failure::TimedOut
	} else {
		Failure::Execution(Error::External("inference failed".into()))
	};
	settle(
		repository.as_ref(),
		session().id,
		&job.input.message,
		Err(failure),
	)
	.await
	.unwrap();
	let state = repository.state.lock().unwrap();
	let outcome = state.finished.as_ref().unwrap();
	assert_eq!(outcome.status, status);
	assert_eq!(outcome.tool_calls, prior);
	assert_eq!(outcome.conversation, session().conversation.unwrap());
	assert_eq!(outcome.usage, session().usage);
	assert_eq!(
		outcome.error.as_deref(),
		Some(if !timeout {
			"inference failed"
		} else if unknown {
			"test timed out while an external call was in flight; its outcome is unknown"
		} else {
			"model request timed out; provider outcome is unknown"
		})
	);
	assert_eq!(state.order, ["snapshot", "finish"]);
}
#[tokio::test]
async fn missing_payload_uses_only_the_original_message_and_finish_preserves_stop() {
	let repository = Arc::new(Repository::new());
	let job = job(model(repository.clone(), vec![]));
	{
		let mut state = repository.state.lock().unwrap();
		state.session.conversation = None;
		state.session.tool_calls = None;
	}
	settle(
		repository.as_ref(),
		session().id,
		&job.input.message,
		Err(Failure::TimedOut),
	)
	.await
	.unwrap();
	{
		let state = repository.state.lock().unwrap();
		assert_eq!(
			state.finished.as_ref().unwrap().conversation,
			json!([{"role":"user","content":"original"}])
		);
		assert_eq!(state.finished.as_ref().unwrap().tool_calls, json!([]));
	}
	{
		let mut state = repository.state.lock().unwrap();
		state.session.status = "stopped".into();
		state.finished = None;
	}
	settle(
		repository.as_ref(),
		session().id,
		&job.input.message,
		Err(Failure::TimedOut),
	)
	.await
	.unwrap();
	let state = repository.state.lock().unwrap();
	assert_eq!(state.session.status, "stopped");
	assert!(state.finished.is_none());
}
#[tokio::test]
async fn cancelled_inference_has_no_completion_or_progress_and_no_held_lease() {
	let repository = Arc::new(Repository::new());
	let model = Arc::new(Model {
		repository: repository.clone(),
		responses: Mutex::new(VecDeque::new()),
		requests: Mutex::new(vec![]),
		failure: false,
		pause: true,
	});
	let job = job(model.clone());
	let execution = execution(repository.clone());
	{
		let operation = simulate(&execution, session().id, &job);
		tokio::pin!(operation);
		assert!(futures_util::poll!(&mut operation).is_pending());
	}
	let state = repository.state.lock().unwrap();
	assert!(!state.active);
	assert!(state.finished.is_none());
	assert_eq!(state.session.tool_calls, Some(json!([])));
	assert_eq!(model.requests.lock().unwrap().len(), 1);
}

#[rstest]
#[case::scope("file_search", json!({"scope":{"scope":["allowed"]}}), json!({"scope":"fixture"}), true)]
#[case::limit("file_search", json!({"limits":{"limit":4}}), json!({"scope":"fixture","limit":5}), true)]
#[case::host("outbound_get", json!({"allowed_hosts":["allowed.example"]}), json!({"url":"https://denied.example"}), true)]
#[case::defaults("file_search", json!({"limits":{"limit":4}}), json!({"scope":"fixture"}), false)]
#[tokio::test]
async fn simulated_fixtures_obey_the_admitted_binding(
	#[case] operation: &str,
	#[case] narrow: Value,
	#[case] arguments: Value,
	#[case] denied: bool,
) {
	let mut repository = Repository::new();
	let mut binding =
		crate::test_support::binding("tool", "aidash://local", &format!("aidash.{operation}"));
	binding["narrow"] = narrow;
	repository.entry.config["bindings"] = json!([binding]);
	let snapshot = crate::test_support::resolve("aidash://local", &repository.entry, false, vec![]);
	repository.state.get_mut().unwrap().session.scenario["binding_snapshot"] = json!(snapshot);
	let repository = Arc::new(repository);
	let mut answer = response(true);
	answer.tool_calls[0].name = operation.into();
	answer.tool_calls[0].arguments = arguments;
	let model = model(repository.clone(), vec![answer, response(false)]);
	let mut job = job(model.clone());
	job.input.fixtures.insert(
		operation.into(),
		Fixture {
			status: FixtureStatus::Success,
			response: json!({"fixture":true}),
		},
	);
	let outcome = simulate(&execution(repository.clone()), session().id, &job)
		.await
		.unwrap();
	assert_eq!(outcome.status, if denied { "blocked" } else { "completed" });
	assert_eq!(
		outcome.tool_calls[0]["outcome"],
		if denied { "denied" } else { "simulated" }
	);
	assert_eq!(
		model.requests.lock().unwrap().len(),
		if denied { 1 } else { 2 }
	);
	if denied {
		assert!(outcome.error.as_ref().unwrap().contains("Binding"));
		assert!(outcome.tool_calls[0].get("fixture").is_none());
	} else {
		assert_eq!(outcome.tool_calls[0]["arguments"]["limit"], 4);
	}
	assert_eq!(
		repository
			.state
			.lock()
			.unwrap()
			.session
			.tool_calls
			.as_ref()
			.unwrap(),
		&outcome.tool_calls
	);
}
#[tokio::test]
async fn fixture_cannot_enable_an_unbound_tool_alias() {
	let repository = Arc::new(Repository::new());
	let mut answer = response(true);
	answer.tool_calls[0].name = "unbound".into();
	let model = model(repository.clone(), vec![answer]);
	let mut job = job(model.clone());
	job.input.fixtures.insert(
		"unbound".into(),
		Fixture {
			status: FixtureStatus::Success,
			response: json!({"fixture":true}),
		},
	);
	let outcome = simulate(&execution(repository), session().id, &job)
		.await
		.unwrap();
	assert_eq!(outcome.status, "blocked");
	assert_eq!(outcome.tool_calls[0]["outcome"], "denied");
	assert_eq!(model.requests.lock().unwrap().len(), 1);
}
