//! Agent lifecycle and inference authority are tested without a database or HTTP.
use super::*;
use aidash_application::ports::{
	CompactionClassifier, CompactionQuestions, InferenceProgressSink, ModelProvider,
};
use aidash_domain::provider::progress::{
	FLUSH_INTERVAL, InferenceProgress, MAX_PENDING_BYTES, MAX_PROGRESS_ITEM_BYTES,
};
use aidash_domain::provider::{ContentPart, ModelRequest, ModelResponse, ToolCall};
use aidash_domain::registry::{EntityRef, Entry};
use aidash_domain::run_input::RunInput;
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One scripted action of the fake provider during a single inference.
enum Step {
	Offer(InferenceProgress),
	Sleep(Duration),
	/// Accept a new Run input, as an operator correction would.
	Input(&'static str),
	Interrupt(InferenceInterruption),
	Fail(Error),
}
fn text(value: &str) -> Step {
	Step::Offer(InferenceProgress::Text { text: value.into() })
}

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
	/// Provider scripts consumed one per inference; an empty queue answers at once.
	scripts: Mutex<VecDeque<Vec<Step>>>,
	attempts: Mutex<Vec<(InferenceAttemptId, Vec<ProgressOutcome>)>>,
	progress: Mutex<Vec<(InferenceAttemptId, Vec<InferenceProgress>)>>,
	interruption: tokio::sync::watch::Sender<Option<InferenceInterruption>>,
	inputs: Mutex<Vec<RunInput>>,
	messages: Mutex<Vec<Message>>,
	deny_recheck: bool,
	fail_append: bool,
}
impl Backend {
	fn record(&self, name: &'static str) {
		self.0.calls.lock().unwrap().push(name);
	}
	fn script(&self, steps: Vec<Step>) {
		self.0.scripts.lock().unwrap().push_back(steps);
	}
	fn outcomes(&self) -> Vec<Vec<ProgressOutcome>> {
		let attempts = self.0.attempts.lock().unwrap();
		attempts
			.iter()
			.map(|(_, outcomes)| outcomes.clone())
			.collect()
	}
	fn progress_text(&self) -> String {
		let progress = self.0.progress.lock().unwrap();
		progress
			.iter()
			.flat_map(|(_, batch)| batch)
			.filter_map(|item| match item {
				InferenceProgress::Text { text } => Some(text.as_str()),
				InferenceProgress::ToolCall { .. } => None,
			})
			.collect()
	}
	fn calls(&self) -> Vec<&'static str> {
		self.0.calls.lock().unwrap().clone()
	}
	fn events(&self) -> Vec<String> {
		let writes = self.0.writes.lock().unwrap();
		writes.iter().map(|(event, _)| event.clone()).collect()
	}
	fn add_input(&self, content: &str) {
		let task = self.task_value();
		let mut inputs = self.0.inputs.lock().unwrap();
		let id = Uuid::new_v4();
		let seq = inputs.len() as i64 + 1;
		inputs.push(RunInput {
			seq,
			sender: "human".into(),
			content: content.into(),
			idempotency_key: id.to_string(),
			message_id: Some(id),
			reference_only: false,
		});
		self.0.messages.lock().unwrap().push(Message {
			id,
			workspace_id: task.workspace_id,
			sender: "human".into(),
			content: content.into(),
			idempotency_key: None,
			created_at: task.created_at,
		});
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
		if event == "model.completed" {
			// The store closes the pending attempt as accepted in the same transaction.
			let mut attempts = self.0.attempts.lock().unwrap();
			if let Some((_, outcomes)) = attempts.iter_mut().rfind(|(_, o)| o.is_empty()) {
				outcomes.push(ProgressOutcome::Accepted);
			}
		}
		self.0
			.writes
			.lock()
			.unwrap()
			.push((event.to_owned(), run.clone()));
		Ok(())
	}
	async fn start_inference(
		&self,
		run: &Run,
		token: Uuid,
		attempt: InferenceAttemptId,
	) -> Result<()> {
		let _ = run;
		assert_eq!(token, self.0.token);
		self.record("inference.start");
		self.0.attempts.lock().unwrap().push((attempt, vec![]));
		Ok(())
	}
	async fn append_inference_progress(
		&self,
		run: &Run,
		token: Uuid,
		attempt: InferenceAttemptId,
		batch: &[InferenceProgress],
	) -> Result<()> {
		let _ = run;
		assert_eq!(token, self.0.token);
		self.record("inference.append");
		assert!(
			self.0
				.attempts
				.lock()
				.unwrap()
				.iter()
				.any(|(id, outcomes)| *id == attempt && outcomes.is_empty()),
			"progress is appended only to the pending attempt"
		);
		if self.0.fail_append {
			return Err(Error::External("progress store unavailable".into()));
		}
		self.0
			.progress
			.lock()
			.unwrap()
			.push((attempt, batch.to_vec()));
		Ok(())
	}
	async fn finish_inference(
		&self,
		run: &Run,
		token: Uuid,
		attempt: InferenceAttemptId,
		outcome: ProgressOutcome,
	) -> Result<()> {
		let _ = run;
		assert_eq!(token, self.0.token);
		self.record("inference.finish");
		let mut attempts = self.0.attempts.lock().unwrap();
		let (_, outcomes) = attempts
			.iter_mut()
			.find(|(id, _)| *id == attempt)
			.expect("finished attempt was started");
		outcomes.push(outcome);
		Ok(())
	}
	async fn emit(&self, workspace: Option<Uuid>, kind: &str, data: Value) -> Result<Event> {
		let _ = (workspace, kind, data);
		unexpected("ExecutionStore.emit")
	}
	async fn run_inputs(&self, run: Uuid) -> Result<Vec<aidash_domain::run_input::RunInput>> {
		let _ = run;
		Ok(self.0.inputs.lock().unwrap().clone())
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
		let _ = (run, token, key, name, input, replay_safe);
		unexpected("ExecutionStore.invocation_start")
	}
	async fn invocation_finish(
		&self,
		run: &Run,
		token: Uuid,
		key: &str,
		output: &Value,
	) -> Result<()> {
		let _ = (run, token, key, output);
		unexpected("ExecutionStore.invocation_finish")
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
		let _ = messages;
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

	fn skill_instructions(&self, _entry: &Entry) -> Result<String> {
		unexpected("skill_instructions")
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
			let messages = self.0.messages.lock().unwrap();
			let message = messages.iter().find(|m| m.id.to_string() == id);
			return Ok(json!(message.expect("stored message")));
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
		false
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
	async fn recheck_inference(&self) -> Result<()> {
		self.record("authority.recheck");
		if self.0.deny_recheck {
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
		unexpected("ExecutionEnvironment.skill_context")
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
	async fn wait_for_inference_interruption(
		&self,
		run: Uuid,
		included_input_seq: i64,
	) -> Result<InferenceInterruption> {
		let _ = (run, included_input_seq);
		let mut signal = self.0.interruption.subscribe();
		let interruption = signal
			.wait_for(Option::is_some)
			.await
			.map(|value| value.expect("signalled interruption"))
			.expect("interruption sender lives with the backend");
		self.0.interruption.send_replace(None);
		Ok(interruption)
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
			skills: vec![],
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
	async fn infer(
		&self,
		request: ModelRequest,
		progress: &dyn InferenceProgressSink,
	) -> Result<ModelResponse> {
		request.ensure_fits(128000).unwrap();
		self.record("provider.infer");
		self.0.requests.lock().unwrap().push(request);
		if let Some(status) = self.0.provider_status {
			return Err(Error::ProviderRejected {
				status,
				reason: "Fixture rejection".into(),
			});
		}
		let script = self
			.0
			.scripts
			.lock()
			.unwrap()
			.pop_front()
			.unwrap_or_default();
		for step in script {
			match step {
				Step::Offer(item) => progress.offer(item),
				Step::Sleep(duration) => tokio::time::sleep(duration).await,
				Step::Input(content) => self.add_input(content),
				Step::Interrupt(interruption) => {
					self.0.interruption.send_replace(Some(interruption));
				}
				Step::Fail(error) => return Err(error),
			}
		}
		self.record("provider.response");
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
		scripts: Mutex::new(VecDeque::new()),
		attempts: Mutex::new(vec![]),
		progress: Mutex::new(vec![]),
		interruption: tokio::sync::watch::channel(None).0,
		inputs: Mutex::new(vec![]),
		messages: Mutex::new(vec![]),
		deny_recheck: false,
		fail_append: false,
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
				|| name.starts_with("inference.")
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
			"inference.start",
			"provider.infer",
			"authority.suspend",
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
	drop(calls);
	// Output rejected after the provider call closes its attempt as revoked.
	let expected = if resumed {
		vec![vec![ProgressOutcome::Interrupted(
			InterruptionReason::Revoked,
		)]]
	} else {
		vec![]
	};
	assert_eq!(fixture.backend.outcomes(), expected);
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
		Ok(Tools::new())
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

/// Counts metric updates made on the test thread; histograms count recordings.
#[derive(Default)]
struct Metrics(Mutex<BTreeMap<String, Arc<Count>>>);
#[derive(Default)]
struct Count(AtomicU64);
impl metrics::CounterFn for Count {
	fn increment(&self, value: u64) {
		self.0.fetch_add(value, Ordering::Relaxed);
	}
	fn absolute(&self, value: u64) {
		self.0.fetch_max(value, Ordering::Relaxed);
	}
}
impl metrics::HistogramFn for Count {
	fn record(&self, _: f64) {
		self.0.fetch_add(1, Ordering::Relaxed);
	}
}
impl Metrics {
	fn handle(&self, key: &metrics::Key) -> Arc<Count> {
		let mut name = key.name().to_owned();
		for label in key.labels() {
			name.push_str(&format!("{{{}={}}}", label.key(), label.value()));
		}
		self.0.lock().unwrap().entry(name).or_default().clone()
	}
	fn get(&self, name: &str) -> u64 {
		let counts = self.0.lock().unwrap();
		counts
			.get(name)
			.map_or(0, |count| count.0.load(Ordering::Relaxed))
	}
}
impl metrics::Recorder for Metrics {
	fn describe_counter(
		&self,
		_: metrics::KeyName,
		_: Option<metrics::Unit>,
		_: metrics::SharedString,
	) {
	}
	fn describe_gauge(
		&self,
		_: metrics::KeyName,
		_: Option<metrics::Unit>,
		_: metrics::SharedString,
	) {
	}
	fn describe_histogram(
		&self,
		_: metrics::KeyName,
		_: Option<metrics::Unit>,
		_: metrics::SharedString,
	) {
	}
	fn register_counter(&self, key: &metrics::Key, _: &metrics::Metadata<'_>) -> metrics::Counter {
		metrics::Counter::from_arc(self.handle(key))
	}
	fn register_gauge(&self, _: &metrics::Key, _: &metrics::Metadata<'_>) -> metrics::Gauge {
		metrics::Gauge::noop()
	}
	fn register_histogram(
		&self,
		key: &metrics::Key,
		_: &metrics::Metadata<'_>,
	) -> metrics::Histogram {
		metrics::Histogram::from_arc(self.handle(key))
	}
}

fn thinking(mut fixture: Fixture) -> Fixture {
	Arc::get_mut(&mut fixture.backend.0).unwrap().scoped = true;
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	fixture
}
fn position(calls: &[&str], name: &str) -> usize {
	calls
		.iter()
		.position(|call| *call == name)
		.unwrap_or_else(|| panic!("{name} was not called"))
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn streamed_progress_is_stored_before_the_response_is_accepted(fixture: Fixture) {
	// Arrange
	let metrics = Metrics::default();
	let _recorder = metrics::set_default_local_recorder(&metrics);
	let mut fixture = thinking(fixture);
	fixture.backend.script(vec![
		text("Completed "),
		Step::Sleep(FLUSH_INTERVAL * 2),
		text("task"),
	]);
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	let calls = fixture.backend.calls();
	assert!(position(&calls, "inference.append") < position(&calls, "provider.response"));
	assert!(position(&calls, "inference.start") < position(&calls, "provider.infer"));
	assert!(calls.contains(&"reservation.settle"));
	let response = &fixture.run.state.tool().unwrap().response;
	assert_eq!(fixture.backend.progress_text(), response.text);
	assert_eq!(fixture.backend.outcomes(), [[ProgressOutcome::Accepted]]);
	assert_eq!(fixture.backend.events().last().unwrap(), "model.completed");
	assert_eq!(metrics.get("aidash_inference_first_delta_seconds"), 1);
	assert_eq!(metrics.get("aidash_inference_accepted_seconds"), 1);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn cancellation_mid_stream_interrupts_without_settling_or_tool_state(fixture: Fixture) {
	// Arrange
	let metrics = Metrics::default();
	let _recorder = metrics::set_default_local_recorder(&metrics);
	let mut fixture = thinking(fixture);
	fixture.backend.script(vec![
		text("Partial"),
		Step::Sleep(FLUSH_INTERVAL * 2),
		Step::Interrupt(InferenceInterruption::Cancelled),
		Step::Sleep(Duration::from_secs(60)),
	]);
	// Act
	let result = advance_sources(&mut fixture).await;
	// Assert
	assert!(matches!(result, Err(Error::Conflict(_))));
	assert_eq!(fixture.backend.progress_text(), "Partial");
	assert_eq!(
		fixture.backend.outcomes(),
		[[ProgressOutcome::Interrupted(InterruptionReason::Cancelled)]]
	);
	let calls = fixture.backend.calls();
	assert!(!calls.contains(&"reservation.settle"));
	assert!(!calls.contains(&"provider.response"));
	assert_eq!(fixture.run.phase(), RunPhase::Thinking);
	assert!(
		!fixture
			.backend
			.events()
			.contains(&"model.completed".to_owned())
	);
	assert_eq!(
		metrics.get("aidash_inference_interruptions_total{reason=cancelled}"),
		1
	);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn new_input_mid_stream_discards_the_attempt_and_replans_with_it(fixture: Fixture) {
	// Arrange
	let metrics = Metrics::default();
	let _recorder = metrics::set_default_local_recorder(&metrics);
	let mut fixture = thinking(fixture);
	fixture.backend.script(vec![
		Step::Offer(InferenceProgress::ToolCall {
			index: 0,
			id: Some("call-1".into()),
			name: Some("workspace_write".into()),
			argument_bytes: 12,
		}),
		Step::Sleep(FLUSH_INTERVAL * 2),
		Step::Input("Use the revised plan"),
		Step::Interrupt(InferenceInterruption::Superseded),
		Step::Sleep(Duration::from_secs(60)),
	]);
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	assert_eq!(fixture.run.phase(), RunPhase::Thinking);
	assert_eq!(fixture.run.step, 0);
	assert_eq!(
		fixture.backend.events().last().unwrap(),
		"run.message_received"
	);
	assert_eq!(fixture.backend.outcomes(), [[ProgressOutcome::Discarded]]);
	let calls = fixture.backend.calls();
	assert!(!calls.contains(&"reservation.settle"));
	assert!(!calls.contains(&"provider.response"));
	assert!(
		!fixture
			.backend
			.events()
			.contains(&"model.completed".to_owned())
	);
	assert_eq!(
		metrics.get("aidash_inference_interruptions_total{reason=correction}"),
		1
	);

	// Act: the next step plans again and includes the new input.
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	let requests = fixture.backend.0.requests.lock().unwrap();
	assert!(!format!("{:?}", requests[0]).contains("Use the revised plan"));
	assert!(format!("{:?}", requests[1]).contains("Use the revised plan"));
	drop(requests);
	assert_eq!(fixture.run.phase(), RunPhase::ToolCall);
	assert_eq!(
		fixture.backend.outcomes(),
		[[ProgressOutcome::Discarded], [ProgressOutcome::Accepted]]
	);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn revoked_authority_mid_stream_interrupts_and_stops_progress(fixture: Fixture) {
	// Arrange
	let metrics = Metrics::default();
	let _recorder = metrics::set_default_local_recorder(&metrics);
	let mut fixture = thinking(fixture);
	Arc::get_mut(&mut fixture.backend.0).unwrap().deny_recheck = true;
	fixture.backend.script(
		(0..50)
			.flat_map(|_| [text("chunk "), Step::Sleep(Duration::from_millis(100))])
			.collect(),
	);
	// Act
	let result = advance_sources(&mut fixture).await;
	// Assert
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(
		fixture.backend.outcomes(),
		[[ProgressOutcome::Interrupted(InterruptionReason::Revoked)]]
	);
	let calls = fixture.backend.calls();
	let recheck = position(&calls, "authority.recheck");
	assert!(calls[..recheck].contains(&"inference.append"));
	assert!(!calls[recheck..].contains(&"inference.append"));
	assert!(!calls.contains(&"reservation.settle"));
	assert!(!calls.contains(&"provider.response"));
	assert_eq!(fixture.run.phase(), RunPhase::Thinking);
	assert_eq!(
		metrics.get("aidash_inference_interruptions_total{reason=revoked}"),
		1
	);
}

#[rstest]
#[case::stall(Error::InferenceStalled, InterruptionReason::Stall)]
#[case::stream_error(Error::External("connection reset".into()), InterruptionReason::StreamError)]
#[tokio::test(start_paused = true)]
async fn failed_streams_store_their_progress_then_interrupt(
	fixture: Fixture,
	#[case] error: Error,
	#[case] reason: InterruptionReason,
) {
	// Arrange
	let mut fixture = thinking(fixture);
	let expected = std::mem::discriminant(&error);
	fixture
		.backend
		.script(vec![text("Partial"), Step::Fail(error)]);
	// Act
	let result = advance_sources(&mut fixture).await;
	// Assert
	assert_eq!(std::mem::discriminant(&result.unwrap_err()), expected);
	assert_eq!(fixture.backend.progress_text(), "Partial");
	let calls = fixture.backend.calls();
	assert!(position(&calls, "inference.append") < position(&calls, "inference.finish"));
	assert!(!calls.contains(&"reservation.settle"));
	assert_eq!(
		fixture.backend.outcomes(),
		[[ProgressOutcome::Interrupted(reason)]]
	);
	assert_eq!(fixture.run.phase(), RunPhase::Thinking);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn flooding_progress_stays_bounded_and_counts_drops(fixture: Fixture) {
	// Arrange
	let metrics = Metrics::default();
	let _recorder = metrics::set_default_local_recorder(&metrics);
	let mut fixture = thinking(fixture);
	let escaped = "\"".repeat(20 * 1024);
	let chunk = "x".repeat(1024);
	let mut script = vec![text(&escaped)];
	script.extend((0..1024).map(|_| text(&chunk)));
	fixture.backend.script(script);
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	assert_eq!(fixture.backend.progress_text().len(), MAX_PENDING_BYTES);
	let accepted_chunks = (MAX_PENDING_BYTES - escaped.len()) / chunk.len();
	assert_eq!(
		metrics.get("aidash_inference_progress_coalesced_total"),
		(1024 - accepted_chunks) as u64
	);
	let progress = fixture.backend.0.progress.lock().unwrap();
	assert!(
		progress
			.iter()
			.flat_map(|(_, batch)| batch)
			.all(|item| serde_json::to_vec(item).unwrap().len() <= MAX_PROGRESS_ITEM_BYTES)
	);
	drop(progress);
	assert_eq!(fixture.backend.outcomes(), [[ProgressOutcome::Accepted]]);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn failed_progress_append_stops_display_but_not_inference(fixture: Fixture) {
	// Arrange
	let mut fixture = thinking(fixture);
	Arc::get_mut(&mut fixture.backend.0).unwrap().fail_append = true;
	fixture.backend.script(vec![
		text("Completed "),
		Step::Sleep(FLUSH_INTERVAL * 2),
		text("task"),
		Step::Sleep(FLUSH_INTERVAL * 2),
	]);
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	let calls = fixture.backend.calls();
	assert_eq!(
		calls
			.iter()
			.filter(|call| **call == "inference.append")
			.count(),
		1
	);
	assert_eq!(fixture.run.phase(), RunPhase::ToolCall);
	assert_eq!(fixture.backend.outcomes(), [[ProgressOutcome::Accepted]]);
}
