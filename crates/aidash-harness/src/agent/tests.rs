//! Agent lifecycle and inference authority are tested without a database or HTTP.
use super::*;
use aidash_application::ports::{CompactionClassifier, CompactionQuestions, ModelProvider};
use aidash_domain::capabilities::skills::SkillMetadata;
use aidash_domain::exposure::{
	CapabilityKind, DirectSkill, ExposureState, ExposureUpdate, Loaded, SkillOriginKind,
};
use aidash_domain::provider::{ContentPart, ModelRequest, ModelResponse, ToolCall, ToolSpec};
use aidash_domain::registry::bindings::{
	BindingOrigin, DEFAULT_TOOLS, EXPOSURE_TOOLS, Narrowing, QualifiedRef, REQUIRED_TOOLS,
	ResolvedBinding, ResolvedDefinition, SKILL_ASSET_READ, SKILL_TOOLS,
};
use aidash_domain::registry::{EntityRef, Entry, rules};
use aidash_domain::tool::providers::core_descriptor;
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Backend(Arc<State>);
struct State {
	conversation_memory: bool,
	deny_source: bool,
	memory_value: Mutex<Value>,
	requests: Mutex<Vec<ModelRequest>>,
	token: Uuid,
	task: Mutex<Task>,
	calls: Mutex<Vec<&'static str>>,
	writes: Mutex<Vec<(String, Run)>>,
	scoped: bool,
	deny_inference: bool,
	deny_resume: bool,
	permit_completion: bool,
	provider_status: Option<u16>,
	dependency_status: Option<TaskStatus>,
	remote_home: bool,
	human: Mutex<Option<HumanRequest>>,
	tools: Vec<&'static str>,
	skills: Vec<EntityRef>,
	skill_context: Option<String>,
	local_authority: bool,
	inputs: Vec<aidash_domain::run_input::RunInput>,
	messages: Vec<Message>,
	/// Tools that replace or extend the builtin fixtures, by alias.
	custom: BTreeMap<String, Arc<Scripted>>,
	/// Journaled invocations by key: status and result.
	invocations: Mutex<BTreeMap<String, (String, Option<Value>)>>,
	direct_skills: Vec<DirectSkill>,
	direct_bodies: BTreeMap<Uuid, String>,
}
impl Backend {
	fn record(&self, name: &'static str) {
		self.0.calls.lock().unwrap().push(name);
	}
	fn task_value(&self) -> Task {
		self.0.task.lock().unwrap().clone()
	}
	fn entry(&self, id: &str) -> Entry {
		serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":"agent","name":{"en":id},"description":{"en":"Fixture"},"config":{
        "provider":"openrouter","model_id":"fixture","endpoint":"http://fixture.invalid/v1","credential_env":null,"context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}
    }})).unwrap()
	}
}
fn unexpected(operation: &str) -> ! {
	panic!("unexpected port call: {operation}")
}

#[async_trait]
impl ExecutionStore for Backend {
	async fn save_run(&self, run: &Run, token: Uuid, event: &str) -> Result<()> {
		assert_eq!(token, self.0.token);
		self.record("save");
		self.0
			.writes
			.lock()
			.unwrap()
			.push((event.to_owned(), run.clone()));
		Ok(())
	}
	async fn emit(&self, workspace: Option<Uuid>, kind: &str, data: Value) -> Result<Event> {
		let _ = (workspace, kind, data);
		unexpected("ExecutionStore.emit")
	}
	async fn run_inputs(&self, run: Uuid) -> Result<Vec<aidash_domain::run_input::RunInput>> {
		let _ = run;
		Ok(self.0.inputs.clone())
	}
	async fn begin_final_completion(&self, run: &Run, token: Uuid) -> Result<bool> {
		let _ = run;
		assert_eq!(token, self.0.token);
		self.record("completion.fence");
		Ok(self.0.permit_completion)
	}
	async fn human_request(
		&self,
		run: &Run,
		kind: &str,
		prompt: &str,
		key: &str,
	) -> Result<HumanRequest> {
		let _ = (run, kind, prompt, key);
		unexpected("ExecutionStore.human_request")
	}
	async fn human_request_by_id(&self, id: Uuid) -> Result<HumanRequest> {
		let _ = id;
		unexpected("ExecutionStore.human_request_by_id")
	}
	async fn expire_workbench_approval(&self, id: Uuid) -> Result<HumanRequest> {
		let _ = id;
		unexpected("ExecutionStore.expire_workbench_approval")
	}
	async fn invocation_start(
		&self,
		run: &Run,
		token: Uuid,
		key: &str,
		name: &str,
		input: &Value,
		replay_safe: bool,
	) -> Result<InvocationOutcome> {
		let _ = (run, name, input, replay_safe);
		assert_eq!(token, self.0.token);
		self.record("invocation.start");
		let mut invocations = self.0.invocations.lock().unwrap();
		let (status, result) = invocations
			.entry(key.to_owned())
			.or_insert_with(|| ("STARTED".into(), None))
			.clone();
		Ok(InvocationOutcome { status, result })
	}
	async fn invocation_finish(
		&self,
		run: &Run,
		token: Uuid,
		key: &str,
		output: &Value,
	) -> Result<()> {
		let _ = run;
		assert_eq!(token, self.0.token);
		self.record("invocation.finish");
		self.0
			.invocations
			.lock()
			.unwrap()
			.insert(key.to_owned(), ("COMPLETED".into(), Some(output.clone())));
		Ok(())
	}
	async fn reconciliation_request(
		&self,
		run: &mut Run,
		token: Uuid,
		key: &str,
		prompt: &str,
	) -> Result<()> {
		let _ = (run, token, key, prompt);
		unexpected("ExecutionStore.reconciliation_request")
	}
	async fn run_message_has_media(&self, messages: &[Uuid]) -> Result<bool> {
		assert!(messages.iter().all(|id| {
			self.0
				.inputs
				.iter()
				.any(|input| input.message_id == Some(*id))
		}));
		Ok(false)
	}
}

#[async_trait]
impl ExecutionCatalog for Backend {
	async fn get_for_run(&self, run: &Run, id: &str, version: &str) -> Result<Entry> {
		let _ = (run, version);
		self.record("catalog");
		Ok(self.entry(id))
	}

	fn skill_instructions(&self, entry: &Entry) -> Result<String> {
		assert!(
			self.0.skills.iter().any(|skill| skill.id == entry.id),
			"unexpected skill_instructions for {}",
			entry.id
		);
		Ok(format!(
			"# {0}\nApply the {0} checklist to every change.\n",
			entry.id
		))
	}
	fn content_digest(&self, content: &str) -> String {
		aidash_domain::registry::rules::digest(&json!(content))
	}
}

#[async_trait]
impl ExecutionHome for Backend {
	async fn human_request_by_id(&self, id: Uuid) -> Result<HumanRequest> {
		self.record("home.human_read");
		let request = self.0.human.lock().unwrap().clone().expect("Home request");
		assert_eq!(request.id, id);
		Ok(request)
	}
	async fn task(&self) -> Result<Task> {
		self.record("home.task");
		Ok(self.task_value())
	}
	async fn claim(&self, task: &Task, agent: &Entry) -> Result<Task> {
		let _ = (task, agent);
		self.record("home.claim");
		Ok(self.task_value())
	}
	async fn transition(&self, next: TaskStatus) -> Result<Task> {
		self.record("home.transition");
		self.0.task.lock().unwrap().status = next;
		Ok(self.task_value())
	}
	async fn read_record(&self, kind: &str, id: &str) -> Result<Value> {
		if kind == "message" {
			let message = self
				.0
				.messages
				.iter()
				.find(|message| message.id.to_string() == id)
				.expect("fixture message");
			return Ok(json!(message));
		}
		assert_eq!(kind, "task");
		let mut task = self.task_value();
		task.id = id.parse().unwrap();
		task.status = self.0.dependency_status.expect("dependency status");
		Ok(json!(task))
	}
	async fn read_record_chunk(
		&self,
		kind: &str,
		id: &str,
		offset: usize,
		maximum: usize,
	) -> Result<Value> {
		let _ = (kind, id, offset, maximum);
		unexpected("ExecutionHome.read_record_chunk")
	}
	async fn observation(&self, offset: usize, limit: usize) -> Result<Value> {
		let _ = (offset, limit);
		Ok(json!({"workspace_id":self.task_value().workspace_id}))
	}
	async fn observation_fitted(
		&self,
		offset: usize,
		limit: usize,
		fits: &ObservationFit<'_>,
	) -> Result<Option<(usize, Value)>> {
		let _ = (offset, limit, fits);
		unexpected("ExecutionHome.observation_fitted")
	}
	async fn child_summary(&self, parent: Uuid) -> Result<ChildTaskSummary> {
		let _ = parent;
		Ok(ChildTaskSummary {
			has_pending: false,
			has_failed: false,
		})
	}
	async fn complete(&self, key: &str, artifact: &ArtifactInput) -> Result<Task> {
		let _ = (key, artifact);
		self.record("home.complete");
		self.0.task.lock().unwrap().status = TaskStatus::Completed;
		Ok(self.task_value())
	}
	async fn report(&self, key: &str, kind: &str, data: Value) -> Result<()> {
		let _ = (key, kind, data);
		self.record("home.report");
		Ok(())
	}
	async fn response_message(
		&self,
		token: Uuid,
		input_sequence: i64,
		key: &str,
		text: &str,
	) -> Result<()> {
		let _ = (input_sequence, key, text);
		assert_eq!(token, self.0.token);
		self.record("home.output");
		Ok(())
	}

	fn local(&self) -> bool {
		!self.0.remote_home
	}
	fn has_local_authority(&self) -> bool {
		self.0.local_authority
	}
}

#[async_trait]
impl ExecutionAuthority for Backend {
	async fn action(&self, action: &str, kind: &str, id: Uuid) -> Result<()> {
		let _ = (action, kind, id);
		self.record("authority.action");
		Ok(())
	}
	async fn inference(&self) -> Result<()> {
		self.record("authority.inference");
		if self.0.deny_inference {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn tool(
		&self,
		call: &ToolCall,
		_contract: &aidash_domain::tool::ToolContract,
	) -> Result<()> {
		let _ = call;
		unexpected("ExecutionAuthority.tool")
	}
	async fn human_read(&self, id: Uuid) -> Result<()> {
		let _ = id;
		self.record("authority.human_read");
		Ok(())
	}
	async fn model_media(&self, selections: &[Selection]) -> Result<Vec<ContentPart>> {
		let _ = selections;
		unexpected("ExecutionAuthority.model_media")
	}
	async fn human_message_media(
		&self,
		messages: &[(i64, Uuid, usize)],
		model: &ModelConfig,
	) -> Result<HumanMediaBatch> {
		let _ = (messages, model);
		unexpected("ExecutionAuthority.human_message_media")
	}
	async fn reserve_inference(
		&self,
		token: Uuid,
		window: usize,
		output: u32,
		request: &ModelRequest,
	) -> Result<Option<Box<dyn InferenceReservation>>> {
		let _ = (window, output, request);
		assert_eq!(token, self.0.token);
		self.record("reservation.admit");
		Ok(Some(Box::new(self.clone())))
	}
	async fn suspend(&self) -> Result<()> {
		self.record("authority.suspend");
		Ok(())
	}
	async fn resume(&self) -> Result<()> {
		self.record("authority.resume");
		if self.0.deny_resume {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	fn is_remote(&self) -> bool {
		false
	}
}

#[async_trait]
impl ExecutionEnvironment for Backend {
	fn binding_resolver(&self) -> &dyn aidash_application::ports::bindings::BindingResolver {
		self
	}
	async fn documents(&self, entry: &Entry) -> Result<Value> {
		let _ = entry;
		Ok(json!([]))
	}
	async fn recheck_source_observation(&self, _: &Run, _: &Value) -> Result<()> {
		if self.0.conversation_memory {
			self.record("source.recheck");
		}
		if self.0.deny_source {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn skill_context(&self, run: &Run) -> Result<String> {
		let _ = run;
		Ok(self
			.0
			.skill_context
			.clone()
			.unwrap_or_else(|| unexpected("ExecutionEnvironment.skill_context")))
	}
	async fn direct_skills(&self, run: &Run) -> Result<Vec<DirectSkill>> {
		let _ = run;
		self.record("skills.direct");
		Ok(self.0.direct_skills.clone())
	}
	async fn direct_skill_body(&self, run: &Run, skill_id: Uuid, digest: &str) -> Result<String> {
		let _ = run;
		self.record("skills.direct_body");
		let skill = self
			.0
			.direct_skills
			.iter()
			.find(|skill| skill.metadata.skill_id == skill_id)
			.expect("pinned direct Skill");
		if skill.metadata.digest != digest {
			return Err(Error::Conflict("CAPABILITY_CHANGED".into()));
		}
		Ok(self.0.direct_bodies[&skill_id].clone())
	}
	async fn semantic_context(
		&self,
		run: &Run,
		task: &Task,
		inputs: &[(InputRead, String)],
		budget: usize,
		entry: &Entry,
	) -> Result<Option<Value>> {
		let _ = (run, task, inputs, budget, entry);
		if self.0.conversation_memory {
			self.record("source.memory");
			return Ok(Some(self.0.memory_value.lock().unwrap().clone()));
		}
		Ok(None)
	}
	async fn run_message_limit(&self, run: &Run) -> Result<usize> {
		let _ = run;
		Ok(16000)
	}
	async fn run_request_headroom(&self, run: &Run) -> Result<usize> {
		let _ = run;
		Ok(16000)
	}
	async fn deliver_run_messages(&self, run: &Run) -> Result<()> {
		let _ = run;
		self.record("inputs.deliver");
		Ok(())
	}
	async fn reconcile_run_messages(&self, run: &Run) -> Result<()> {
		let _ = run;
		self.record("inputs.reconcile");
		Ok(())
	}
	async fn require_terminal_safe_delivery(&self, run: &Run) -> Result<()> {
		let _ = run;
		Ok(())
	}
	async fn transition_terminal_run_messages(&self, run: &Run, status: TaskStatus) -> Result<()> {
		let _ = (run, status);
		unexpected("ExecutionEnvironment.transition_terminal_run_messages")
	}
	async fn wait_for_inference_cancellation(&self, run: Uuid) -> Result<()> {
		let _ = run;
		std::future::pending().await
	}
	async fn operator_human_message_media(
		&self,
		run: &Run,
		messages: &[(i64, Uuid, usize)],
		model: &ModelConfig,
	) -> Result<HumanMediaBatch> {
		let _ = (run, messages, model);
		unexpected("ExecutionEnvironment.operator_human_message_media")
	}

	fn node_id(&self) -> &str {
		"aidash://fixture"
	}
	fn store(&self) -> &dyn ExecutionStore {
		self
	}
	fn catalog(&self) -> &dyn ExecutionCatalog {
		self
	}
	fn authority(&self) -> Option<&dyn ExecutionAuthority> {
		self.0.scoped.then_some(self)
	}
	fn home(&self, _run: &Run) -> Box<dyn ExecutionHome> {
		Box::new(self.clone())
	}
	fn agent(&self, _entry: &Entry) -> Result<ExecutionAgent> {
		Ok(ExecutionAgent {
			model: EntityRef {
				id: "model".into(),
				version: "1.0.0".into(),
			},
			instructions: "Do the task".into(),
			knowledge_digest: None,
			tools: vec![],
			skills: self.0.skills.clone(),
			max_steps: 64,
			allow_task_creation: None,
			conversation_memory: self.0.conversation_memory,
		})
	}
	fn provider(&self, _model: ModelConfig) -> Result<Arc<dyn ModelProvider>> {
		Ok(Arc::new(self.clone()))
	}
	fn compactor(&self) -> Result<Box<dyn CompactionClassifier>> {
		Ok(Box::new(self.clone()))
	}
}

#[async_trait]
impl ExecutionVisibility for Backend {
	async fn suspend(&mut self) -> Result<()> {
		self.record("visibility.suspend");
		Ok(())
	}
	async fn resume(&mut self) -> Result<()> {
		self.record("visibility.resume");
		Ok(())
	}
}

#[async_trait]
impl ModelProvider for Backend {
	async fn infer(&self, request: ModelRequest) -> Result<ModelResponse> {
		request.ensure_fits(128000).unwrap();
		self.record("provider.infer");
		self.0.requests.lock().unwrap().push(request);
		if let Some(status) = self.0.provider_status {
			return Err(Error::ProviderRejected {
				status,
				reason: "Fixture rejection".into(),
			});
		}
		Ok(ModelResponse {
			text: "Completed task".into(),
			input_tokens: 200,
			output_tokens: 10,
			usage_complete: true,
			..Default::default()
		})
	}
}
#[async_trait]
impl CompactionClassifier for Backend {
	async fn ask(&self, _state: &Value, _questions: &CompactionQuestions) -> Result<Value> {
		unexpected("compaction")
	}
}
#[async_trait]
impl InferenceReservation for Backend {
	async fn settle(self: Box<Self>, response: &ModelResponse) -> Result<()> {
		assert!(response.usage_complete);
		self.record("reservation.settle");
		Ok(())
	}
}
struct Fixture {
	backend: Backend,
	run: Run,
}
#[fixture]
fn fixture() -> Fixture {
	let now = "2026-10-02T00:00:00Z".parse().unwrap();
	let token = Uuid::new_v4();
	let workspace = Uuid::new_v4();
	let task = Uuid::new_v4();
	let run = Run {
		id: Uuid::new_v4(),
		task_id: task,
		workspace_id: workspace,
		home_node: "aidash://fixture".into(),
		agent_id: "agent".into(),
		agent_version: "1.0.0".into(),
		state_version: StateVersion::default(),
		state: RunState::Ready(ReadyState::default()),
		recovery: RecoveryState::default(),
		control: RunControl::Active,
		context: Context::default(),
		step: 0,
		revision: 0,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		error: None,
		lease_owner: Some(token),
		lease_until: Some(now),
		updated_at: now,
	};
	let backend = Backend(Arc::new(State {
		conversation_memory: false,
		deny_source: false,
		memory_value: Mutex::new(json!({"fact":"observed"})),
		requests: Mutex::new(vec![]),
		token,
		task: Mutex::new(Task {
			id: task,
			workspace_id: workspace,
			title: "Task".into(),
			description: "Fixture task".into(),
			status: TaskStatus::Open,
			requirements: json!({}),
			owner: None,
			created_by: "human".into(),
			dependencies: vec![],
			parent_id: None,
			revision: 0,
			created_at: now,
		}),
		calls: Mutex::new(vec![]),
		writes: Mutex::new(vec![]),
		scoped: false,
		deny_inference: false,
		deny_resume: false,
		permit_completion: true,
		provider_status: None,
		dependency_status: None,
		remote_home: false,
		human: Mutex::new(None),
		tools: vec![],
		skills: vec![],
		skill_context: None,
		local_authority: false,
		inputs: vec![],
		messages: vec![],
		custom: BTreeMap::new(),
		invocations: Mutex::new(BTreeMap::new()),
		direct_skills: vec![],
		direct_bodies: BTreeMap::new(),
	}));
	Fixture { backend, run }
}

#[rstest]
#[tokio::test]
async fn remote_human_poll_preserves_waiting_until_home_answers(mut fixture: Fixture) {
	let id = Uuid::new_v4();
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.remote_home = true;
	state.scoped = true;
	*state.human.lock().unwrap() = Some(HumanRequest {
		id,
		workspace_id: fixture.run.workspace_id,
		run_id: fixture.run.id,
		kind: "QUESTION".into(),
		prompt: "Continue?".into(),
		response: None,
		created_at: fixture.run.updated_at,
		answered_by: None,
	});
	fixture.run.state = RunState::Waiting(Box::new(WaitingState::Human {
		request_id: id,
		resume: ResumeState::Ready(ReadyState::default()),
	}));
	for _ in 0..2 {
		Executor::new(&fixture.backend)
			.advance(
				&mut fixture.run,
				fixture.backend.0.token,
				&mut fixture.backend.clone(),
			)
			.await
			.unwrap();
		assert!(
			matches!(&fixture.run.state, RunState::Waiting(wait) if wait.request_id() == Some(id))
		);
	}
	assert!(
		fixture
			.backend
			.0
			.writes
			.lock()
			.unwrap()
			.iter()
			.all(|(event, run)| event == "run.waiting" && run.phase() == RunPhase::Waiting)
	);
	fixture
		.backend
		.0
		.human
		.lock()
		.unwrap()
		.as_mut()
		.unwrap()
		.response = Some(json!({"answer":"Continue"}));
	Executor::new(&fixture.backend)
		.advance(
			&mut fixture.run,
			fixture.backend.0.token,
			&mut fixture.backend.clone(),
		)
		.await
		.unwrap();
	assert_eq!(fixture.run.phase(), RunPhase::Ready);
	assert_eq!(
		fixture.backend.0.writes.lock().unwrap().last().unwrap().0,
		"run.resumed"
	);
	let calls = fixture.backend.0.calls.lock().unwrap();
	assert_eq!(
		calls
			.iter()
			.filter(|call| **call == "authority.human_read")
			.count(),
		3
	);
	assert!(!calls.contains(&"provider.infer"));
}
#[rstest]
#[case(TaskStatus::Completed, RunPhase::Completed)]
#[case(TaskStatus::Failed, RunPhase::Failed)]
#[case(TaskStatus::Cancelled, RunPhase::Cancelled)]
#[case(TaskStatus::Abandoned, RunPhase::Cancelled)]
#[tokio::test]
async fn terminal_home_status_is_reconciled_without_another_provider_call(
	mut fixture: Fixture,
	#[case] status: TaskStatus,
	#[case] phase: RunPhase,
) {
	// Arrange
	fixture.backend.0.task.lock().unwrap().status = status;
	// Act
	Executor::new(&fixture.backend)
		.advance(
			&mut fixture.run,
			fixture.backend.0.token,
			&mut fixture.backend.clone(),
		)
		.await
		.unwrap();
	// Assert
	assert_eq!(fixture.run.phase(), phase);
	assert_eq!(
		*fixture.backend.0.calls.lock().unwrap(),
		["inputs.deliver", "home.task", "save"]
	);
	assert_eq!(fixture.backend.0.writes.lock().unwrap().len(), 1);
}
#[rstest]
#[case(TaskStatus::Open, false)]
#[case(TaskStatus::Claimed, false)]
#[case(TaskStatus::Running, false)]
#[case(TaskStatus::Failed, true)]
#[case(TaskStatus::Cancelled, true)]
#[case(TaskStatus::Abandoned, true)]
#[tokio::test]
async fn unfinished_or_failed_dependencies_prevent_claim_and_inference(
	mut fixture: Fixture,
	#[case] status: TaskStatus,
	#[case] failed: bool,
) {
	// Arrange
	fixture
		.backend
		.0
		.task
		.lock()
		.unwrap()
		.dependencies
		.push(Uuid::new_v4());
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.dependency_status = Some(status);
	// Act
	let result = Executor::new(&fixture.backend)
		.advance(
			&mut fixture.run,
			fixture.backend.0.token,
			&mut fixture.backend.clone(),
		)
		.await;
	// Assert
	assert!(
		!fixture
			.backend
			.0
			.calls
			.lock()
			.unwrap()
			.contains(&"home.claim")
	);
	assert!(
		!fixture
			.backend
			.0
			.calls
			.lock()
			.unwrap()
			.contains(&"provider.infer")
	);
	if failed {
		assert!(matches!(result, Err(Error::Invalid(_))));
		assert!(fixture.backend.0.writes.lock().unwrap().is_empty());
	} else {
		result.unwrap();
		assert!(
			matches!(fixture.run.state,RunState::Waiting(ref state) if matches!(state.as_ref(),WaitingState::Dependencies{..}))
		);
	}
}
#[rstest]
#[tokio::test]
async fn inference_rechecks_authority_and_settles_usage_before_accepting_output(
	mut fixture: Fixture,
) {
	// Arrange
	Arc::get_mut(&mut fixture.backend.0).unwrap().scoped = true;
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	// Act
	Executor::new(&fixture.backend)
		.advance(
			&mut fixture.run,
			fixture.backend.0.token,
			&mut fixture.backend.clone(),
		)
		.await
		.unwrap();
	// Assert
	let calls = fixture.backend.0.calls.lock().unwrap();
	let boundary = calls
		.iter()
		.copied()
		.filter(|name| {
			name.starts_with("authority.")
				|| name.starts_with("visibility.")
				|| name.starts_with("reservation.")
				|| *name == "provider.infer"
				|| *name == "save"
		})
		.collect::<Vec<_>>();
	assert_eq!(
		boundary,
		[
			"authority.inference",
			"save",
			"authority.inference",
			"reservation.admit",
			"authority.suspend",
			"visibility.suspend",
			"provider.infer",
			"visibility.resume",
			"reservation.settle",
			"authority.resume",
			"save"
		]
	);
	assert_eq!(fixture.run.phase(), RunPhase::ToolCall);
	assert_eq!(
		fixture.backend.0.writes.lock().unwrap().last().unwrap().0,
		"model.completed"
	);
}
#[rstest]
#[case::before_provider(true, false)]
#[case::after_provider(false, true)]
#[tokio::test]
async fn rejected_authority_never_accepts_or_persists_provider_output(
	mut fixture: Fixture,
	#[case] initial: bool,
	#[case] resumed: bool,
) {
	// Arrange
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = true;
	state.deny_inference = initial;
	state.deny_resume = resumed;
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	// Act
	let result = Executor::new(&fixture.backend)
		.advance(
			&mut fixture.run,
			fixture.backend.0.token,
			&mut fixture.backend.clone(),
		)
		.await;
	// Assert
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(fixture.run.phase(), RunPhase::Thinking);
	assert!(
		fixture
			.backend
			.0
			.writes
			.lock()
			.unwrap()
			.iter()
			.all(|(event, saved)| event == "run.sources_observed"
				&& saved.phase() == RunPhase::Thinking)
	);
	let calls = fixture.backend.0.calls.lock().unwrap();
	assert_eq!(calls.contains(&"provider.infer"), resumed);
	assert_eq!(calls.contains(&"reservation.settle"), resumed);
}
#[rstest]
#[tokio::test]
async fn late_run_input_prevents_final_completion_and_output_publication(mut fixture: Fixture) {
	// Arrange
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.permit_completion = false;
	fixture.run.state = RunState::ToolCall(Box::new(ToolCallState {
		response: ModelResponse {
			text: "Stale answer".into(),
			..Default::default()
		},
		..Default::default()
	}));
	// Act
	Executor::new(&fixture.backend)
		.advance(
			&mut fixture.run,
			fixture.backend.0.token,
			&mut fixture.backend.clone(),
		)
		.await
		.unwrap();
	// Assert
	assert_eq!(fixture.run.phase(), RunPhase::Thinking);
	let calls = fixture.backend.0.calls.lock().unwrap();
	assert!(calls.contains(&"completion.fence"));
	assert!(!calls.contains(&"home.output"));
	assert!(!calls.contains(&"home.complete"));
	assert_eq!(
		fixture.backend.0.writes.lock().unwrap()[0].0,
		"run.message_received"
	);
}
#[rstest]
#[tokio::test]
async fn ready_inference_and_completion_use_the_same_durable_executor(mut fixture: Fixture) {
	// Arrange / Act: separate invocations model separate durable worker steps.
	for _ in 0..3 {
		Executor::new(&fixture.backend)
			.advance(
				&mut fixture.run,
				fixture.backend.0.token,
				&mut fixture.backend.clone(),
			)
			.await
			.unwrap();
	}
	// Assert
	assert_eq!(fixture.run.phase(), RunPhase::Completed);
	assert_eq!(
		fixture.backend.0.task.lock().unwrap().status,
		TaskStatus::Completed
	);
	let writes = fixture.backend.0.writes.lock().unwrap();
	assert_eq!(
		writes
			.iter()
			.map(|(event, _)| event.as_str())
			.collect::<Vec<_>>(),
		[
			"run.started",
			"run.sources_observed",
			"model.completed",
			"run.completed"
		]
	);
	let calls = fixture.backend.0.calls.lock().unwrap();
	assert_eq!(
		calls
			.iter()
			.filter(|&&name| name == "provider.infer")
			.count(),
		1
	);
	assert_eq!(
		calls.iter().filter(|&&name| name == "home.output").count(),
		1
	);
	assert_eq!(
		calls
			.iter()
			.filter(|&&name| name == "home.complete")
			.count(),
		1
	);
}

#[async_trait]
impl aidash_application::ports::bindings::BindingResolver for Backend {
	async fn tools(&self, _: &Run) -> Result<Tools> {
		let mut tools: Tools = self
			.0
			.tools
			.iter()
			.map(|name| ((*name).to_owned(), Advertised::builtin(name)))
			.collect();
		for (alias, tool) in &self.0.custom {
			tools.insert(alias.clone(), tool.clone());
		}
		Ok(tools)
	}
}

/// Advertises the application builtin specification when one exists, so the
/// request baselines pin real builtin descriptions and schemas.
struct Advertised {
	specification: ToolSpec,
	contract: aidash_domain::tool::ToolContract,
}
impl Advertised {
	fn builtin(name: &str) -> Arc<dyn ExecutionTool> {
		let specification = aidash_application::tools::builtins()
			.remove(name)
			.map_or_else(
				|| ToolSpec {
					name: name.into(),
					description: format!("Fixture {name} tool."),
					parameters: json!({"type":"object","additionalProperties":false}),
				},
				|builtin| builtin.specification(),
			);
		Arc::new(Self {
			specification,
			contract: aidash_domain::tool::builtin_contract(name).expect("declared builtin"),
		})
	}
}
#[async_trait]
impl ExecutionTool for Advertised {
	fn specification(&self) -> ToolSpec {
		self.specification.clone()
	}
	fn contract(&self) -> aidash_domain::tool::ToolContract {
		self.contract.clone()
	}
	fn replay_safe(&self) -> bool {
		self.contract.replay_safe()
	}
	async fn invoke(&self, run: &Run, input: Value, key: &str) -> Result<Value> {
		let _ = (run, input, key);
		unexpected("ExecutionTool.invoke")
	}
}

#[rstest]
#[tokio::test]
async fn recovery_reuses_the_observed_memory_then_reloads_at_a_later_boundary(
	mut fixture: Fixture,
) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.conversation_memory = true;
	state.provider_status = Some(503);
	fixture.run.state = RunState::Thinking(ThinkingState::default());

	// The provider failure leaves the source read durable, before any model output.
	assert!(advance_sources(&mut fixture).await.is_err());
	let saved = fixture
		.backend
		.0
		.writes
		.lock()
		.unwrap()
		.last()
		.unwrap()
		.1
		.clone();
	assert!(saved.context.source_observation.is_some());
	fixture.run = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
	*fixture.backend.0.memory_value.lock().unwrap() = json!({"fact":"changed"});
	assert!(advance_sources(&mut fixture).await.is_err());
	{
		let requests = fixture.backend.0.requests.lock().unwrap();
		assert_eq!(requests[0].context, requests[1].context);
	}
	assert_eq!(
		fixture
			.backend
			.0
			.calls
			.lock()
			.unwrap()
			.iter()
			.filter(|c| **c == "source.memory")
			.count(),
		1
	);
	fixture.run.step += 1;
	assert!(advance_sources(&mut fixture).await.is_err());
	let requests = fixture.backend.0.requests.lock().unwrap();
	assert_ne!(requests[1].context, requests[2].context);
	assert_eq!(
		fixture
			.backend
			.0
			.calls
			.lock()
			.unwrap()
			.iter()
			.filter(|c| **c == "source.memory")
			.count(),
		2
	);
}
#[rstest]
#[tokio::test]
async fn source_revocation_stops_recovered_observations_before_inference(mut fixture: Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.conversation_memory = true;
	state.provider_status = Some(503);
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	assert!(
		Executor::new(&fixture.backend)
			.advance(
				&mut fixture.run,
				fixture.backend.0.token,
				&mut fixture.backend.clone()
			)
			.await
			.is_err()
	);
	Arc::get_mut(&mut fixture.backend.0).unwrap().deny_source = true;
	assert!(matches!(
		Executor::new(&fixture.backend)
			.advance(
				&mut fixture.run,
				fixture.backend.0.token,
				&mut fixture.backend.clone()
			)
			.await,
		Err(Error::Forbidden)
	));
	assert_eq!(fixture.backend.0.requests.lock().unwrap().len(), 1);
}

async fn advance_sources(fixture: &mut Fixture) -> Result<()> {
	let backend = fixture.backend.clone();
	Executor::new(&backend)
		.advance(&mut fixture.run, backend.0.token, &mut backend.clone())
		.await
}

fn default_tools() -> Vec<&'static str> {
	REQUIRED_TOOLS
		.iter()
		.chain(DEFAULT_TOOLS)
		.copied()
		.collect()
}
fn plain_agent(fixture: &mut Fixture) {
	Arc::get_mut(&mut fixture.backend.0).unwrap().tools = default_tools();
}
fn registry_skill(fixture: &mut Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.tools = default_tools();
	state.skills = vec![EntityRef {
		id: "code-review".into(),
		version: "1.2.0".into(),
	}];
}
fn direct_skill(fixture: &mut Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.tools = REQUIRED_TOOLS.iter().chain(SKILL_TOOLS).copied().collect();
	state.local_authority = true;
	// The shape of `capabilities::skills::context`: metadata per pinned Skill,
	// followed by SKILL.md for each loaded one.
	state.skill_context = Some(
		concat!(
			"\nPinned Skills (select by UUID and origin; use skill_load):\n",
			r#"{"skill_id":"00000000-0000-0000-0000-000000000031","name":"release-notes","description":"Draft release notes from merged changes.","origin":"area:00000000-0000-0000-0000-000000000030:.agents/skills/release-notes"}"#,
			"\n---\nname: release-notes\ndescription: Draft release notes from merged changes.\n---\n# Release notes\nGroup changes by user impact.\n\n",
			r#"{"skill_id":"00000000-0000-0000-0000-000000000032","name":"triage","description":"Classify incoming issues.","origin":"area:00000000-0000-0000-0000-000000000030:.agents/skills/triage"}"#,
			"\n",
		)
		.into(),
	);
}
fn run_message_catch_up(fixture: &mut Fixture) {
	let id = Uuid::from_u128(0x20);
	let content = "Also include the rollback steps.";
	let workspace_id = fixture.run.workspace_id;
	let created_at = fixture.run.updated_at;
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.tools = default_tools();
	state.inputs = vec![aidash_domain::run_input::RunInput {
		seq: 2,
		sender: "human".into(),
		content: content.into(),
		idempotency_key: "input-2".into(),
		message_id: Some(id),
		reference_only: false,
	}];
	state.messages = vec![Message {
		id,
		workspace_id,
		sender: "human".into(),
		content: content.into(),
		idempotency_key: Some("input-2".into()),
		created_at,
	}];
	// An earlier summary turns every unprocessed run message into catch-up.
	fixture.run.context.run_message_summary = "The user wants a release checklist.".into();
	fixture.run.context.run_message_summary_seq = 1;
	fixture.run.observed_input_seq = 1;
}

/// Legacy Agents (no exposure policy) must keep sending exactly these
/// requests. Baselines are compact `serde_json` output of the captured
/// `ModelRequest` (instructions, context, tools in order, output limit).
#[rstest]
#[case::default_tools(
	plain_agent,
	&default_tools(),
	include_str!("../../../../tests/fixtures/legacy_model_request_default_tools.json")
)]
#[case::registry_skill(
	registry_skill,
	&default_tools(),
	include_str!("../../../../tests/fixtures/legacy_model_request_registry_skill.json")
)]
#[case::direct_skill(
	direct_skill,
	&["workspace_read", "human_request", "skill_list", "skill_load", "skill_read"],
	include_str!("../../../../tests/fixtures/legacy_model_request_direct_skill.json")
)]
#[case::run_message_catch_up(
	run_message_catch_up,
	&["workspace_read"],
	include_str!("../../../../tests/fixtures/legacy_model_request_run_message_catch_up.json")
)]
#[tokio::test]
async fn legacy_thinking_request_matches_byte_baseline(
	mut fixture: Fixture,
	#[case] arrange: fn(&mut Fixture),
	#[case] advertised: &[&str],
	#[case] baseline: &str,
) {
	// Arrange: fixed identities keep the serialized context reproducible.
	let workspace = Uuid::from_u128(0x10);
	let task = Uuid::from_u128(0x11);
	fixture.run.id = Uuid::from_u128(0x12);
	fixture.run.workspace_id = workspace;
	fixture.run.task_id = task;
	{
		let mut home = fixture.backend.0.task.lock().unwrap();
		home.id = task;
		home.workspace_id = workspace;
	}
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	arrange(&mut fixture);
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	let requests = fixture.backend.0.requests.lock().unwrap();
	let [request] = requests.as_slice() else {
		panic!("expected one provider request, got {}", requests.len());
	};
	// Tools are advertised in alias order, after the catch-up filter.
	let mut sorted = advertised.to_vec();
	sorted.sort_unstable();
	assert_eq!(
		request
			.tools
			.iter()
			.map(|tool| tool.name.as_str())
			.collect::<Vec<_>>(),
		sorted
	);
	assert_eq!(serde_json::to_string(request).unwrap(), baseline.trim());
}

/// A tool whose results the test scripts in call order; counts real invocations.
struct Scripted {
	specification: ToolSpec,
	contract: aidash_domain::tool::ToolContract,
	outputs: Mutex<std::collections::VecDeque<Value>>,
	invocations: Mutex<usize>,
}
impl Scripted {
	fn new(specification: ToolSpec, contract: &str) -> Arc<Self> {
		Arc::new(Self {
			specification,
			contract: aidash_domain::tool::builtin_contract(contract).expect("declared builtin"),
			outputs: Mutex::new(Default::default()),
			invocations: Mutex::new(0),
		})
	}
	fn invocations(&self) -> usize {
		*self.invocations.lock().unwrap()
	}
}
#[async_trait]
impl ExecutionTool for Scripted {
	fn specification(&self) -> ToolSpec {
		self.specification.clone()
	}
	fn contract(&self) -> aidash_domain::tool::ToolContract {
		self.contract.clone()
	}
	fn replay_safe(&self) -> bool {
		self.contract.replay_safe()
	}
	async fn invoke(&self, run: &Run, input: Value, key: &str) -> Result<Value> {
		let _ = (run, input, key);
		*self.invocations.lock().unwrap() += 1;
		Ok(self
			.outputs
			.lock()
			.unwrap()
			.pop_front()
			.expect("scripted tool output"))
	}
}

const NODE: &str = "aidash://fixture";
const LARGE_TOOLS: usize = 12;
const DIRECT_SKILL: Uuid = Uuid::from_u128(0x31);

fn reference(id: &str) -> QualifiedRef {
	QualifiedRef {
		registry_node: NODE.into(),
		id: id.into(),
		version: "1.0.0".into(),
	}
}

/// An unvalidated `deferred@1` Run snapshot: the Executor reads only the
/// Agent policy, bindings and Skill definitions from it.
#[derive(Default)]
struct DeferredSnapshot {
	agent: Vec<Value>,
	definitions: Vec<ResolvedDefinition>,
	bindings: Vec<ResolvedBinding>,
}
impl DeferredSnapshot {
	fn define(
		&mut self,
		identity: &QualifiedRef,
		kind: &str,
		description: &str,
		config: Value,
	) -> (Entry, String) {
		let entry: Entry = serde_json::from_value(json!({
			"id": identity.id,
			"version": identity.version,
			"kind": kind,
			"name": {"en": identity.id},
			"description": {"en": description},
			"config": config,
		}))
		.unwrap();
		let digest = rules::digest(&serde_json::to_value(&entry).unwrap());
		self.definitions.push(ResolvedDefinition {
			identity: identity.clone(),
			definition: entry.clone(),
			digest: digest.clone(),
		});
		(entry, digest)
	}
	fn bind(
		&mut self,
		identity: QualifiedRef,
		kind: &str,
		description: &str,
		config: Value,
		alias: Option<&str>,
		origin: BindingOrigin,
	) {
		let (definition, digest) = self.define(&identity, kind, description, config);
		let tool = kind == "tool";
		self.bindings.push(ResolvedBinding {
			identity,
			definition,
			digest,
			origin,
			alias: alias.map(Into::into),
			narrow: Narrowing::default(),
			installation: None,
			provider_contract_digest: tool.then(|| "contract".into()),
			provider_implementation: tool.then(|| "implementation".into()),
			excluded_reason: None,
		});
	}
	fn eager(&mut self, kind: &str, id: &str) {
		self.agent
			.push(json!({"kind": kind, "target": reference(id), "exposure": "eager"}));
	}
	fn snapshot(mut self) -> BindingSnapshot {
		let agent = reference("agent");
		let config = json!({
			"schema_version": 1,
			"model": {"id": "model", "version": "1.0.0"},
			"instructions": "Do the task",
			"bindings": self.agent,
			"exposure": {"version": "deferred@1"},
		});
		self.define(&agent, "agent", "", config);
		BindingSnapshot {
			schema_version: 1,
			agent,
			remote: false,
			bindings: self.bindings,
			definitions: self.definitions,
			foreign_agents: vec![],
		}
	}
}

fn mandatory_tools() -> Vec<&'static str> {
	REQUIRED_TOOLS
		.iter()
		.chain(EXPOSURE_TOOLS)
		.chain(&[SKILL_ASSET_READ])
		.copied()
		.collect()
}
/// Mandatory exposure, one eager tool and one eager Skill, plus deferred
/// tools and Skills whose definitions together exceed every default budget.
fn deferred_agent(fixture: &mut Fixture) {
	let mut snapshot = DeferredSnapshot::default();
	for operation in mandatory_tools() {
		snapshot.bind(
			QualifiedRef::builtin(NODE, operation),
			"tool",
			operation,
			json!(core_descriptor(NODE, operation).unwrap()),
			Some(operation),
			BindingOrigin::Required,
		);
	}
	let mut custom = BTreeMap::new();
	let mut tool = |snapshot: &mut DeferredSnapshot, alias: String, pad: usize| {
		let description = format!("Fixture {alias} tool.");
		snapshot.bind(
			reference(&alias),
			"tool",
			&description,
			json!(core_descriptor(NODE, "outbound_get").unwrap()),
			Some(&alias),
			BindingOrigin::Explicit,
		);
		let specification = ToolSpec {
			name: alias.clone(),
			description,
			parameters: json!({"type":"object","description":format!("schema-{alias}-{}", "x".repeat(pad))}),
		};
		custom.insert(alias, Scripted::new(specification, "outbound_get"));
	};
	for index in 0..LARGE_TOOLS {
		tool(&mut snapshot, format!("tool_{index:02}"), 2_000);
	}
	tool(&mut snapshot, "eager_tool".into(), 0);
	snapshot.eager("tool", "eager_tool");
	for (id, pad) in [
		("guide", 0),
		("skill-a", 12_000),
		("skill-b", 12_000),
		("skill-c", 12_000),
	] {
		snapshot.bind(
			reference(id),
			"skill",
			&format!("The {id} Skill."),
			json!({"instructions": format!("BODY-{id} {}", "y".repeat(pad))}),
			None,
			BindingOrigin::Explicit,
		);
	}
	snapshot.eager("skill", "guide");
	let load = aidash_application::tools::builtins()
		.remove("capability_load")
		.map_or_else(
			|| ToolSpec {
				name: "capability_load".into(),
				description: "Fixture capability_load tool.".into(),
				parameters: json!({"type":"object","additionalProperties":false}),
			},
			|builtin| builtin.specification(),
		);
	custom.insert(
		"capability_load".into(),
		Scripted::new(load, "capability_load"),
	);
	fixture.run.context.binding_snapshot = Some(Box::new(snapshot.snapshot()));
	let body = "---\nname: release-notes\ndescription: Draft release notes.\n---\nBODY-direct Group changes by user impact.\n";
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.tools = mandatory_tools();
	state.custom = custom;
	state.direct_skills = vec![DirectSkill {
		metadata: SkillMetadata {
			skill_id: DIRECT_SKILL,
			name: "release-notes".into(),
			description: "Draft release notes.".into(),
			origin: "area:00000000-0000-0000-0000-000000000030:.agents/skills/release-notes".into(),
			digest: "sha256:direct".into(),
			license: None,
		},
		origin: SkillOriginKind::Attachment,
		body_bytes: json!(body).to_string().len() - 2,
		files: vec![],
	}];
	state.direct_bodies = BTreeMap::from([(DIRECT_SKILL, body.to_owned())]);
}
fn budgets(run: &Run) -> aidash_domain::exposure::DeferredBudgets {
	deferred_exposure(run).unwrap().expect("deferred Agent").1
}
async fn deferred_catalog(fixture: &Fixture) -> Vec<exposure::Capability> {
	let tools =
		aidash_application::ports::bindings::BindingResolver::tools(&fixture.backend, &fixture.run)
			.await
			.unwrap();
	let snapshot = fixture.run.context.binding_snapshot.as_deref().unwrap();
	exposure_catalog(&fixture.backend, &fixture.run, snapshot, &tools)
		.await
		.unwrap()
}
fn alias_of(catalog: &[exposure::Capability], identity: &exposure::CapabilityIdentity) -> String {
	catalog
		.iter()
		.find(|capability| &capability.identity == identity)
		.unwrap()
		.alias
		.clone()
}
/// The `capability_load` result for `alias` against the Run's current state.
fn load_output(catalog: &[exposure::Capability], run: &Run, alias: &str) -> Value {
	let digest = &catalog.iter().find(|c| c.alias == alias).unwrap().digest;
	exposure::load(
		&budgets(run),
		catalog,
		&run.context.exposure,
		alias,
		digest,
		run.step,
	)
	.unwrap()
}
fn call(id: &str, name: &str, arguments: Value) -> ToolCall {
	ToolCall {
		id: id.into(),
		name: name.into(),
		arguments,
	}
}
fn respond(run: &mut Run, calls: Vec<ToolCall>) {
	run.state = RunState::ToolCall(Box::new(ToolCallState {
		response: ModelResponse {
			tool_calls: calls,
			usage_complete: true,
			..Default::default()
		},
		..Default::default()
	}));
}
fn tool_names(request: &ModelRequest) -> Vec<&str> {
	request
		.tools
		.iter()
		.map(|tool| tool.name.as_str())
		.collect()
}
fn last_write(fixture: &Fixture) -> (String, Run) {
	fixture
		.backend
		.0
		.writes
		.lock()
		.unwrap()
		.last()
		.unwrap()
		.clone()
}

#[rstest]
#[tokio::test]
async fn deferred_first_request_advertises_only_mandatory_and_eager_capabilities(
	mut fixture: Fixture,
) {
	// Arrange
	deferred_agent(&mut fixture);
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	let requests = fixture.backend.0.requests.lock().unwrap();
	let [request] = requests.as_slice() else {
		panic!("expected one provider request, got {}", requests.len());
	};
	let mut expected = mandatory_tools();
	expected.push("eager_tool");
	expected.sort_unstable();
	assert_eq!(tool_names(request), expected);
	assert!(expected.contains(&"human_request"));
	let encoded = serde_json::to_string(request).unwrap();
	assert!(
		!encoded.contains("schema-tool_"),
		"deferred schemas stay out of the request"
	);
	for body in [
		"BODY-skill-a",
		"BODY-skill-b",
		"BODY-skill-c",
		"BODY-direct",
	] {
		assert!(
			!encoded.contains(body),
			"{body} must not be resident before a Load"
		);
	}
	assert_eq!(request.instructions.matches("BODY-guide").count(), 1);
	assert!(
		request
			.instructions
			.contains("tool_00 [tool]: Fixture tool_00 tool.")
	);
	assert!(request.instructions.contains("release_notes"));
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "model.completed");
	let usage = saved
		.context
		.usage
		.clone()
		.unwrap()
		.exposure
		.expect("deferred usage");
	assert!(usage.exposed.iter().any(|(alias, _)| alias == "eager_tool"));
	assert!(usage.schema_bytes <= budgets(&saved).schema_bytes);
	assert!(usage.metadata_bytes > 0);
}

#[rstest]
#[tokio::test]
async fn deferred_dispatch_rejects_a_capability_that_is_not_loaded(mut fixture: Fixture) {
	// Arrange
	deferred_agent(&mut fixture);
	respond(&mut fixture.run, vec![call("call-1", "tool_03", json!({}))]);
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	assert!(matches!(
		saved.context.history.last(),
		Some(ContextEvent::Tool { result, .. })
			if result == &json!({"error":"capability tool_03 is not loaded; use capability_load"})
	));
	assert_eq!(fixture.backend.0.custom["tool_03"].invocations(), 0);
	assert!(
		!fixture
			.backend
			.0
			.calls
			.lock()
			.unwrap()
			.contains(&"invocation.start")
	);
	assert_eq!(saved.state.tool().unwrap().cursor, 1);
}

#[rstest]
#[tokio::test]
async fn a_load_exposes_its_capability_only_from_the_next_request(mut fixture: Fixture) {
	// Arrange: one response loads tool_05 and then calls it.
	deferred_agent(&mut fixture);
	let catalog = deferred_catalog(&fixture).await;
	*fixture.backend.0.custom["capability_load"]
		.outputs
		.lock()
		.unwrap() = [load_output(&catalog, &fixture.run, "tool_05")].into();
	respond(
		&mut fixture.run,
		vec![
			call("call-0", "capability_load", json!({"alias":"tool_05"})),
			call("call-1", "tool_05", json!({})),
		],
	);
	// Act: both tool steps.
	advance_sources(&mut fixture).await.unwrap();
	advance_sources(&mut fixture).await.unwrap();
	// Assert: the call is checked against its own request's Exposure set.
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	assert!(matches!(
		saved.context.history.last(),
		Some(ContextEvent::Tool { result, .. })
			if result == &json!({"error":"capability tool_05 was loaded in this response; call it after the next model request"})
	));
	assert_eq!(fixture.backend.0.custom["tool_05"].invocations(), 0);
	assert!(saved.context.exposure.loaded.is_empty());
	assert_eq!(saved.context.exposure.pending.len(), 1);
	// Act: the step transition, then the next inference.
	advance_sources(&mut fixture).await.unwrap();
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	assert!(fixture.run.context.exposure.pending.is_empty());
	assert_eq!(fixture.run.context.exposure.loaded.len(), 1);
	let requests = fixture.backend.0.requests.lock().unwrap();
	let [request] = requests.as_slice() else {
		panic!("expected one provider request, got {}", requests.len());
	};
	assert!(tool_names(request).contains(&"tool_05"));
}

#[rstest]
#[tokio::test]
async fn loads_change_the_next_request_and_skill_bodies_stay_resident_once(mut fixture: Fixture) {
	// Arrange: one response loads a tool, two Skills, and repeats one Skill load.
	deferred_agent(&mut fixture);
	let catalog = deferred_catalog(&fixture).await;
	let skill = alias_of(
		&catalog,
		&exposure::CapabilityIdentity::Registry(reference("skill-a")),
	);
	let direct = alias_of(
		&catalog,
		&exposure::CapabilityIdentity::DirectSkill {
			skill_id: DIRECT_SKILL,
			origin: fixture.backend.0.direct_skills[0].metadata.origin.clone(),
		},
	);
	let skill_load = load_output(&catalog, &fixture.run, &skill);
	*fixture.backend.0.custom["capability_load"]
		.outputs
		.lock()
		.unwrap() = [
		load_output(&catalog, &fixture.run, "tool_05"),
		skill_load.clone(),
		load_output(&catalog, &fixture.run, &direct),
		skill_load,
	]
	.into();
	respond(
		&mut fixture.run,
		["tool_05", skill.as_str(), direct.as_str(), skill.as_str()]
			.iter()
			.enumerate()
			.map(|(index, alias)| {
				call(
					&format!("call-{index}"),
					"capability_load",
					json!({"alias": alias}),
				)
			})
			.collect(),
	);
	// Act: four tool steps, the step transition, then the next inference.
	for _ in 0..6 {
		advance_sources(&mut fixture).await.unwrap();
	}
	// Assert
	assert_eq!(fixture.backend.0.custom["capability_load"].invocations(), 4);
	let loaded = fixture
		.run
		.context
		.exposure
		.loaded
		.iter()
		.map(|loaded| loaded.alias.as_str())
		.collect::<Vec<_>>();
	assert_eq!(loaded, ["tool_05", skill.as_str(), direct.as_str()]);
	let requests = fixture.backend.0.requests.lock().unwrap();
	let [request] = requests.as_slice() else {
		panic!("expected one provider request, got {}", requests.len());
	};
	assert!(tool_names(request).contains(&"tool_05"));
	assert!(!tool_names(request).contains(&"tool_06"));
	assert_eq!(request.instructions.matches("BODY-skill-a").count(), 1);
	assert_eq!(request.instructions.matches("BODY-direct").count(), 1);
	assert!(!request.instructions.contains("BODY-skill-b"));
	assert!(!request.instructions.contains(&format!("{skill} [skill]")));
	let usage = fixture.run.context.usage.clone().unwrap().exposure.unwrap();
	assert!(usage.exposed.iter().any(|(alias, _)| alias == &skill));
	assert!(
		fixture
			.backend
			.0
			.calls
			.lock()
			.unwrap()
			.contains(&"skills.direct_body")
	);
}

#[rstest]
#[tokio::test]
async fn replayed_load_and_resumed_run_keep_the_same_exposure(mut fixture: Fixture) {
	// Arrange: the load completed before the worker restarted.
	deferred_agent(&mut fixture);
	let catalog = deferred_catalog(&fixture).await;
	let output = load_output(&catalog, &fixture.run, "tool_05");
	let update =
		serde_json::from_value::<ExposureUpdate>(output["exposure_update"].clone()).unwrap();
	let mut staged = ExposureState::default();
	staged.stage(update.clone());
	let mut expected = ExposureState::default();
	expected.apply(&update);
	assert_eq!(
		expected.loaded,
		[Loaded {
			alias: "tool_05".into(),
			kind: CapabilityKind::Tool,
			identity: exposure::CapabilityIdentity::Registry(reference("tool_05")),
			digest: catalog
				.iter()
				.find(|c| c.alias == "tool_05")
				.unwrap()
				.digest
				.clone(),
			step: 0,
		}]
	);
	respond(
		&mut fixture.run,
		vec![call(
			"call-0",
			"capability_load",
			json!({"alias":"tool_05"}),
		)],
	);
	fixture.backend.0.invocations.lock().unwrap().insert(
		format!("{}:0:0", fixture.run.id),
		("COMPLETED".into(), Some(output)),
	);
	// Act: replay the completed invocation.
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	assert_eq!(fixture.backend.0.custom["capability_load"].invocations(), 0);
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	assert_eq!(saved.context.exposure, staged);
	// Act: resume from the persisted Run, then infer.
	fixture.run = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
	advance_sources(&mut fixture).await.unwrap();
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	assert_eq!(fixture.run.context.exposure, expected);
	let requests = fixture.backend.0.requests.lock().unwrap();
	let [request] = requests.as_slice() else {
		panic!("expected one provider request, got {}", requests.len());
	};
	assert!(tool_names(request).contains(&"tool_05"));
}

#[rstest]
#[tokio::test]
async fn deferred_run_message_catch_up_advertises_only_workspace_read(mut fixture: Fixture) {
	// Arrange
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	run_message_catch_up(&mut fixture);
	deferred_agent(&mut fixture);
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	let requests = fixture.backend.0.requests.lock().unwrap();
	let [request] = requests.as_slice() else {
		panic!("expected one provider request, got {}", requests.len());
	};
	assert_eq!(tool_names(request), ["workspace_read"]);
}

/// Answers every compaction question with "drop".
struct DropAll;
#[async_trait]
impl CompactionClassifier for DropAll {
	async fn ask(&self, _state: &Value, questions: &CompactionQuestions) -> Result<Value> {
		let answers = questions
			.keys()
			.map(|name| (name.clone(), json!({"noul": 0.0})))
			.collect::<serde_json::Map<_, _>>();
		Ok(json!({"answers": answers}))
	}
}

#[rstest]
#[tokio::test]
async fn compaction_leaves_the_exposure_set_intact() {
	// Arrange
	let mut context = Context::default();
	context.exposure.apply(&ExposureUpdate::Load(Loaded {
		alias: "tool_05".into(),
		kind: CapabilityKind::Tool,
		identity: exposure::CapabilityIdentity::Registry(reference("tool_05")),
		digest: "sha256:tool".into(),
		step: 1,
	}));
	context.exposure.apply(&ExposureUpdate::Unload {
		alias: "eager_tool".into(),
	});
	for index in 0..24 {
		context.history.push(ContextEvent::tool(
			call(&format!("call-{index}"), "tool_05", json!({})),
			json!({"text": "z".repeat(4_000)}),
		));
	}
	let exposure = context.exposure.clone();
	let pinned = json!({});
	let tools = [];
	let mut budget = context::RequestBudget {
		window: usize::MAX,
		instructions: "Do the task",
		tools: &tools,
		max_output_tokens: 100,
	};
	budget.window = budget.request(&context, &pinned).estimated_total_tokens() / 2;
	// Act
	compact_execution(&mut context, &DropAll, &budget, &pinned)
		.await
		.unwrap();
	// Assert
	assert_eq!(context.compactions, 1);
	assert!(context.history.len() < 24);
	assert_eq!(context.exposure, exposure);
}
