//! Tool Batches driven through `Executor::advance` with an in-memory invocation
//! journal, scripted read providers and a shared Node `ToolSlots`.
use super::*;
use aidash_application::ports::bindings::BindingResolver;
use aidash_domain::provider::ToolSpec;
use aidash_domain::tool::{
	Concurrency, ToolContract, builtin_contract,
	concurrency::{ConcurrentCall, Resource, ResourceClaim},
};
use futures_util::{FutureExt, future::BoxFuture};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::time::Duration;
use tokio::sync::{Barrier, Notify, watch};

/// The response epoch of every fixture response; journal keys embed it.
const EPOCH: i64 = 7;
/// How long a call waits for a sibling it must overlap with.
const RENDEZVOUS: Duration = Duration::from_secs(2);
/// Any advance taking longer has deadlocked.
const PATIENCE: Duration = Duration::from_secs(10);

/// What a scripted provider sees for one execution of one call.
struct Call {
	path: String,
	attempt: usize,
}
type Hook = Box<dyn Fn(&Lab, Call) -> BoxFuture<'static, Result<Value>> + Send + Sync>;

/// Observes every provider execution across the Runs and workers of one test.
struct Lab {
	hook: Hook,
	/// `admit:{path}`, `dispatch:{key}` (batched) and `invoke:{key}` (sequential).
	events: Mutex<Vec<String>>,
	attempts: Mutex<BTreeMap<String, usize>>,
	in_flight: AtomicUsize,
	max_in_flight: AtomicUsize,
	/// Executions dropped before their provider returned.
	dropped: AtomicUsize,
	/// The next N admissions fail as if the worker stopped after batch admission.
	admit_failures: AtomicUsize,
	cancel: watch::Sender<bool>,
}
impl Lab {
	fn new(
		hook: impl Fn(&Lab, Call) -> BoxFuture<'static, Result<Value>> + Send + Sync + 'static,
	) -> Arc<Self> {
		Arc::new(Self {
			hook: Box::new(hook),
			events: Mutex::new(vec![]),
			attempts: Mutex::new(BTreeMap::new()),
			in_flight: AtomicUsize::new(0),
			max_in_flight: AtomicUsize::new(0),
			dropped: AtomicUsize::new(0),
			admit_failures: AtomicUsize::new(0),
			cancel: watch::channel(false).0,
		})
	}
	fn echo() -> Arc<Self> {
		Self::new(|_, call| async move { Ok(read(&call.path)) }.boxed())
	}
	/// Each execution holds its slot until another execution joins it.
	fn rendezvous() -> Arc<Self> {
		let barrier = Arc::new(Barrier::new(2));
		Self::new(move |_, call| {
			let barrier = barrier.clone();
			async move {
				tokio::time::timeout(RENDEZVOUS, barrier.wait())
					.await
					.map_err(|_| Error::External(format!("{} never overlapped", call.path)))?;
				Ok(read(&call.path))
			}
			.boxed()
		})
	}
	/// Executions stay in flight long enough for siblings to start.
	fn lingering() -> Arc<Self> {
		Self::new(|_, call| {
			async move {
				tokio::time::sleep(Duration::from_millis(20)).await;
				Ok(read(&call.path))
			}
			.boxed()
		})
	}
	fn events(&self, kind: &str) -> Vec<String> {
		let prefix = format!("{kind}:");
		self.events
			.lock()
			.unwrap()
			.iter()
			.filter_map(|event| event.strip_prefix(&prefix).map(str::to_owned))
			.collect()
	}
	fn attempts(&self, key: &str) -> usize {
		self.attempts.lock().unwrap().get(key).copied().unwrap_or(0)
	}
	fn max_in_flight(&self) -> usize {
		self.max_in_flight.load(SeqCst)
	}
}
fn read(path: &str) -> Value {
	json!({"content": format!("contents of {path}")})
}

/// Counts one execution or result recording in flight until it returns or is
/// dropped; both hold a database connection on a Node.
struct Flight<'a> {
	lab: &'a Lab,
	returned: bool,
}
impl<'a> Flight<'a> {
	fn enter(lab: &'a Lab) -> Self {
		let now = lab.in_flight.fetch_add(1, SeqCst) + 1;
		lab.max_in_flight.fetch_max(now, SeqCst);
		Self {
			lab,
			returned: false,
		}
	}
}
impl Drop for Flight<'_> {
	fn drop(&mut self) {
		self.lab.in_flight.fetch_sub(1, SeqCst);
		if !self.returned {
			self.lab.dropped.fetch_add(1, SeqCst);
		}
	}
}

/// A provider with a real builtin contract and test-chosen Resource Claims.
struct Probe {
	name: &'static str,
	contract: ToolContract,
	claims: Option<ConcurrentCall>,
	lab: Arc<Lab>,
}
impl Probe {
	async fn execute(&self, via: &str, input: Value, key: &str) -> Result<Value> {
		let path = input["path"].as_str().expect("fixture path").to_owned();
		let attempt = {
			let mut attempts = self.lab.attempts.lock().unwrap();
			let attempt = attempts.entry(key.to_owned()).or_default();
			*attempt += 1;
			*attempt
		};
		self.lab.events.lock().unwrap().push(format!("{via}:{key}"));
		let mut flight = Flight::enter(&self.lab);
		let output = (self.lab.hook)(&self.lab, Call { path, attempt }).await;
		flight.returned = true;
		output
	}
}
#[async_trait]
impl ExecutionTool for Probe {
	fn specification(&self) -> ToolSpec {
		ToolSpec {
			name: self.name.into(),
			description: "Fixture read".into(),
			parameters: json!({"type":"object"}),
		}
	}
	fn contract(&self) -> ToolContract {
		self.contract.clone()
	}
	fn replay_safe(&self) -> bool {
		self.contract.replay_safe()
	}
	async fn invoke(&self, _run: &Run, input: Value, key: &str) -> Result<Value> {
		self.execute("invoke", input, key).await
	}
	fn concurrent_call(&self, _input: &Value) -> Option<ConcurrentCall> {
		self.claims.clone()
	}
	async fn admit(&self, _run: &Run, input: Value) -> Result<Value> {
		let path = input["path"].as_str().expect("fixture path");
		self.lab
			.events
			.lock()
			.unwrap()
			.push(format!("admit:{path}"));
		if self
			.lab
			.admit_failures
			.fetch_update(SeqCst, SeqCst, |left| left.checked_sub(1))
			.is_ok()
		{
			return Err(Error::External("worker stopped after admission".into()));
		}
		Ok(input)
	}
	async fn dispatch(&self, _run: &Run, admitted: Value, key: &str) -> Result<Value> {
		self.execute("dispatch", admitted, key).await
	}
}
fn probe(lab: &Arc<Lab>, name: &'static str) -> Arc<dyn ExecutionTool> {
	let working_area = || ConcurrentCall {
		claims: vec![ResourceClaim::shared(Resource::WorkingArea)],
		output_bytes: 64,
	};
	let (contract, claims) = match name {
		"file_read" | "file_search" => (builtin_contract(name).unwrap(), Some(working_area())),
		// A Run pinned before file_read declared SharedRead keeps it Sequential,
		// even though its provider still derives claims.
		"pinned_read" => {
			let mut sequential = builtin_contract("file_read").unwrap();
			sequential.behavior.concurrency = Concurrency::Sequential;
			let pinned = builtin_contract("file_read")
				.unwrap()
				.pinned(&sequential.digest().unwrap())
				.unwrap()
				.expect("pre-concurrency pin is accepted");
			(pinned, Some(working_area()))
		}
		"shell" => (builtin_contract(name).unwrap(), None),
		_ => unreachable!("fixture tool {name}"),
	};
	Arc::new(Probe {
		name,
		contract,
		claims,
		lab: lab.clone(),
	})
}

#[derive(Clone)]
struct Invocation {
	name: String,
	status: &'static str,
	result: Option<Value>,
	replay_safe: bool,
}

/// One worker's view of one Run: store, Home, guard and Node slots in memory.
#[derive(Clone)]
struct Bench(Arc<Desk>);
struct Desk {
	token: Uuid,
	task: Task,
	lab: Arc<Lab>,
	tools: Tools,
	parallelism: usize,
	slots: Option<ToolSlots>,
	denied: Mutex<BTreeSet<String>>,
	journal: Mutex<BTreeMap<String, Invocation>>,
	/// Every batch admission: its keys and the Run persisted with it.
	admissions: Mutex<Vec<(Vec<String>, Run)>>,
	/// Sequential `invocation_start` keys.
	starts: Mutex<Vec<String>>,
	finishes: Mutex<Vec<(String, Value)>>,
	saves: Mutex<Vec<(String, Run)>>,
	reports: Mutex<Vec<String>>,
}
impl Bench {
	fn deny(&self, call: &str) {
		self.0.denied.lock().unwrap().insert(call.into());
	}
	fn seed(&self, key: &str, status: &'static str, result: Option<Value>) {
		self.0.journal.lock().unwrap().insert(
			key.into(),
			Invocation {
				name: "file_read".into(),
				status,
				result,
				replay_safe: true,
			},
		);
	}
	fn status(&self, key: &str) -> (&'static str, Option<Value>) {
		let journal = self.0.journal.lock().unwrap();
		let invocation = journal.get(key).expect("journaled invocation");
		(invocation.status, invocation.result.clone())
	}
	fn admitted(&self) -> Vec<Vec<String>> {
		let admissions = self.0.admissions.lock().unwrap();
		admissions.iter().map(|(keys, _)| keys.clone()).collect()
	}
	/// The Run as the admission transaction persisted it.
	fn admitted_run(&self) -> Run {
		self.0.admissions.lock().unwrap().last().unwrap().1.clone()
	}
	fn starts(&self) -> Vec<String> {
		self.0.starts.lock().unwrap().clone()
	}
	fn finished(&self) -> Vec<String> {
		let finishes = self.0.finishes.lock().unwrap();
		finishes.iter().map(|(key, _)| key.clone()).collect()
	}
	fn saved(&self) -> Vec<String> {
		let saves = self.0.saves.lock().unwrap();
		saves.iter().map(|(event, _)| event.clone()).collect()
	}
	fn reports(&self) -> Vec<String> {
		self.0.reports.lock().unwrap().clone()
	}
}
fn bench(
	lab: &Arc<Lab>,
	names: &[&'static str],
	parallelism: usize,
	slots: Option<ToolSlots>,
) -> (Bench, Run) {
	let now = "2026-10-02T00:00:00Z".parse().unwrap();
	let token = Uuid::new_v4();
	let workspace = Uuid::new_v4();
	let task = Uuid::new_v4();
	let tool_calls = names
		.iter()
		.enumerate()
		.map(|(index, name)| ToolCall {
			id: format!("call-{index}"),
			name: (*name).into(),
			arguments: json!({"path": format!("p{index}")}),
		})
		.collect();
	let run = Run {
		id: Uuid::new_v4(),
		task_id: task,
		workspace_id: workspace,
		home_node: "aidash://fixture".into(),
		agent_id: "agent".into(),
		agent_version: "1.0.0".into(),
		state_version: StateVersion::default(),
		state: RunState::ToolCall(Box::new(ToolCallState {
			response: ModelResponse {
				tool_calls,
				..Default::default()
			},
			response_epoch: EPOCH,
			request_window: 1_000_000,
			..Default::default()
		})),
		recovery: RecoveryState::default(),
		control: RunControl::Active,
		context: Context::default(),
		step: 3,
		revision: 3,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		error: None,
		lease_owner: Some(token),
		lease_until: Some(now),
		updated_at: now,
	};
	let tools = names
		.iter()
		.map(|name| ((*name).to_owned(), probe(lab, name)))
		.collect();
	let desk = Desk {
		token,
		task: Task {
			id: task,
			workspace_id: workspace,
			title: "Task".into(),
			description: "Fixture task".into(),
			status: TaskStatus::Running,
			requirements: json!({}),
			owner: None,
			created_by: "human".into(),
			dependencies: vec![],
			parent_id: None,
			revision: 0,
			created_at: now,
		},
		lab: lab.clone(),
		tools,
		parallelism,
		slots,
		denied: Mutex::new(BTreeSet::new()),
		journal: Mutex::new(BTreeMap::new()),
		admissions: Mutex::new(vec![]),
		starts: Mutex::new(vec![]),
		finishes: Mutex::new(vec![]),
		saves: Mutex::new(vec![]),
		reports: Mutex::new(vec![]),
	};
	(Bench(Arc::new(desk)), run)
}
fn key(run: &Run, index: usize) -> String {
	format!("{}:{EPOCH}:{index}", run.id)
}
fn keys(run: &Run, indexes: impl IntoIterator<Item = usize>) -> Vec<String> {
	indexes.into_iter().map(|index| key(run, index)).collect()
}
/// The adopted Context Journal: call ids and results, in history order.
fn adopted(run: &Run) -> Vec<(String, Value)> {
	run.context
		.history
		.iter()
		.filter_map(|event| match event {
			ContextEvent::Tool { call, result } => Some((call.id.clone(), result.clone())),
			_ => None,
		})
		.collect()
}
fn expected(results: &[(usize, Value)]) -> Vec<(String, Value)> {
	results
		.iter()
		.map(|(index, result)| (format!("call-{index}"), result.clone()))
		.collect()
}
/// `(cursor, batch_end)` of the Run's ToolCall state.
fn progress(run: &Run) -> (usize, Option<usize>) {
	let state = run.state.tool().unwrap();
	(state.cursor, state.batch_end)
}
async fn advance(bench: &Bench, run: &mut Run) -> Result<()> {
	tokio::time::timeout(
		PATIENCE,
		Executor::new(bench).advance(run, bench.0.token, &mut bench.clone()),
	)
	.await
	.expect("advance finishes without deadlock")
}

#[async_trait]
impl ExecutionStore for Bench {
	async fn save_run(&self, run: &Run, token: Uuid, event: &str) -> Result<()> {
		assert_eq!(token, self.0.token);
		self.0
			.saves
			.lock()
			.unwrap()
			.push((event.to_owned(), run.clone()));
		Ok(())
	}
	async fn emit(&self, _workspace: Option<Uuid>, _kind: &str, _data: Value) -> Result<Event> {
		unexpected("ExecutionStore.emit")
	}
	async fn run_inputs(&self, _run: Uuid) -> Result<Vec<aidash_domain::run_input::RunInput>> {
		Ok(vec![])
	}
	async fn begin_final_completion(&self, _run: &Run, _token: Uuid) -> Result<bool> {
		unexpected("ExecutionStore.begin_final_completion")
	}
	async fn human_request(
		&self,
		_run: &Run,
		_kind: &str,
		_prompt: &str,
		_key: &str,
	) -> Result<HumanRequest> {
		unexpected("ExecutionStore.human_request")
	}
	async fn human_request_by_id(&self, _id: Uuid) -> Result<HumanRequest> {
		unexpected("ExecutionStore.human_request_by_id")
	}
	async fn expire_workbench_approval(&self, _id: Uuid) -> Result<HumanRequest> {
		unexpected("ExecutionStore.expire_workbench_approval")
	}
	async fn invocation_start(
		&self,
		_run: &Run,
		token: Uuid,
		key: &str,
		name: &str,
		_input: &Value,
		replay_safe: bool,
	) -> Result<InvocationOutcome> {
		assert_eq!(token, self.0.token);
		self.0.starts.lock().unwrap().push(key.into());
		let mut journal = self.0.journal.lock().unwrap();
		if let Some(existing) = journal.get(key) {
			assert_eq!(existing.name, name);
			let status = if existing.status == "STARTED" && !existing.replay_safe {
				"UNCERTAIN"
			} else {
				existing.status
			};
			return Ok(InvocationOutcome {
				status: status.into(),
				result: existing.result.clone(),
			});
		}
		journal.insert(
			key.into(),
			Invocation {
				name: name.into(),
				status: "STARTED",
				result: None,
				replay_safe,
			},
		);
		Ok(InvocationOutcome {
			status: "STARTED".into(),
			result: None,
		})
	}
	async fn invocation_start_batch(
		&self,
		run: &Run,
		token: Uuid,
		calls: &[BatchInvocation<'_>],
	) -> Result<Vec<InvocationOutcome>> {
		assert_eq!(token, self.0.token);
		let state = run.state.tool().unwrap();
		assert_eq!(
			state.batch_end,
			Some(state.cursor + calls.len()),
			"admission persists the batch end with the journal rows"
		);
		self.0.admissions.lock().unwrap().push((
			calls.iter().map(|call| call.key.to_owned()).collect(),
			run.clone(),
		));
		let mut journal = self.0.journal.lock().unwrap();
		Ok(calls
			.iter()
			.map(|call| {
				let invocation = journal.entry(call.key.into()).or_insert(Invocation {
					name: call.name.into(),
					status: "STARTED",
					result: None,
					replay_safe: true,
				});
				assert_eq!(invocation.name, call.name);
				InvocationOutcome {
					status: invocation.status.into(),
					result: invocation.result.clone(),
				}
			})
			.collect())
	}
	async fn invocation_finish(
		&self,
		_run: &Run,
		token: Uuid,
		key: &str,
		output: &Value,
	) -> Result<()> {
		assert_eq!(token, self.0.token);
		let mut flight = Flight::enter(&self.0.lab);
		{
			let mut journal = self.0.journal.lock().unwrap();
			let invocation = journal
				.get_mut(key)
				.expect("finished invocation was started");
			assert_eq!(invocation.status, "STARTED", "{key} finished twice");
			invocation.status = "COMPLETED";
			invocation.result = Some(output.clone());
			self.0
				.finishes
				.lock()
				.unwrap()
				.push((key.into(), output.clone()));
		}
		// The recording connection stays busy after the row is written.
		tokio::time::sleep(Duration::from_millis(10)).await;
		flight.returned = true;
		Ok(())
	}
	async fn reconciliation_request(
		&self,
		_run: &mut Run,
		_token: Uuid,
		_key: &str,
		_prompt: &str,
	) -> Result<()> {
		unexpected("ExecutionStore.reconciliation_request")
	}
	async fn run_message_has_media(&self, _messages: &[Uuid]) -> Result<bool> {
		Ok(false)
	}
}

#[async_trait]
impl ExecutionCatalog for Bench {
	async fn get_for_run(&self, _run: &Run, id: &str, _version: &str) -> Result<Entry> {
		Ok(serde_json::from_value(
			json!({"id":id,"version":"1.0.0","kind":"agent","name":{"en":id},"description":{"en":"Fixture"},"config":{
				"provider":"openrouter","model_id":"fixture","endpoint":"http://fixture.invalid/v1","credential_env":null,"context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}
			}}),
		)?)
	}
	fn skill_instructions(&self, _entry: &Entry) -> Result<String> {
		unexpected("ExecutionCatalog.skill_instructions")
	}
	fn content_digest(&self, content: &str) -> String {
		aidash_domain::registry::rules::digest(&json!(content))
	}
}

#[async_trait]
impl ExecutionHome for Bench {
	fn local(&self) -> bool {
		true
	}
	fn has_local_authority(&self) -> bool {
		false
	}
	async fn task(&self) -> Result<Task> {
		Ok(self.0.task.clone())
	}
	async fn claim(&self, _task: &Task, _agent: &Entry) -> Result<Task> {
		unexpected("ExecutionHome.claim")
	}
	async fn transition(&self, _next: TaskStatus) -> Result<Task> {
		unexpected("ExecutionHome.transition")
	}
	async fn read_record(&self, _kind: &str, _id: &str) -> Result<Value> {
		unexpected("ExecutionHome.read_record")
	}
	async fn read_record_chunk(
		&self,
		_kind: &str,
		_id: &str,
		_offset: usize,
		_maximum: usize,
	) -> Result<Value> {
		unexpected("ExecutionHome.read_record_chunk")
	}
	async fn observation(&self, _offset: usize, _limit: usize) -> Result<Value> {
		unexpected("ExecutionHome.observation")
	}
	async fn observation_fitted(
		&self,
		_offset: usize,
		_limit: usize,
		_fits: &ObservationFit<'_>,
	) -> Result<Option<(usize, Value)>> {
		unexpected("ExecutionHome.observation_fitted")
	}
	async fn child_summary(&self, _parent: Uuid) -> Result<ChildTaskSummary> {
		unexpected("ExecutionHome.child_summary")
	}
	async fn complete(&self, _key: &str, _artifact: &ArtifactInput) -> Result<Task> {
		unexpected("ExecutionHome.complete")
	}
	async fn report(&self, key: &str, kind: &str, _data: Value) -> Result<()> {
		assert_eq!(kind, "remote.tool.completed");
		self.0.reports.lock().unwrap().push(key.into());
		Ok(())
	}
	async fn response_message(
		&self,
		_token: Uuid,
		_input_sequence: i64,
		_key: &str,
		_text: &str,
	) -> Result<()> {
		unexpected("ExecutionHome.response_message")
	}
}

#[async_trait]
impl ExecutionAuthority for Bench {
	fn is_remote(&self) -> bool {
		false
	}
	async fn action(&self, action: &str, _kind: &str, _id: Uuid) -> Result<()> {
		unexpected(action)
	}
	async fn inference(&self) -> Result<()> {
		unexpected("ExecutionAuthority.inference")
	}
	async fn tool(&self, call: &ToolCall, _contract: &ToolContract) -> Result<()> {
		if self.0.denied.lock().unwrap().contains(&call.id) {
			return Err(Error::Invalid(format!("{} revoked", call.id)));
		}
		Ok(())
	}
	async fn human_read(&self, _id: Uuid) -> Result<()> {
		unexpected("ExecutionAuthority.human_read")
	}
	async fn model_media(&self, _selections: &[Selection]) -> Result<Vec<ContentPart>> {
		unexpected("ExecutionAuthority.model_media")
	}
	async fn human_message_media(
		&self,
		_messages: &[(i64, Uuid, usize)],
		_model: &ModelConfig,
	) -> Result<HumanMediaBatch> {
		unexpected("ExecutionAuthority.human_message_media")
	}
	async fn reserve_inference(
		&self,
		_token: Uuid,
		_window: usize,
		_output: u32,
		_request: &ModelRequest,
	) -> Result<Option<Box<dyn InferenceReservation>>> {
		unexpected("ExecutionAuthority.reserve_inference")
	}
	async fn suspend(&self) -> Result<()> {
		unexpected("ExecutionAuthority.suspend")
	}
	async fn resume(&self) -> Result<()> {
		unexpected("ExecutionAuthority.resume")
	}
}

#[async_trait]
impl ExecutionVisibility for Bench {
	async fn suspend(&mut self) -> Result<()> {
		unexpected("ExecutionVisibility.suspend")
	}
	async fn resume(&mut self) -> Result<()> {
		unexpected("ExecutionVisibility.resume")
	}
}

#[async_trait]
impl BindingResolver for Bench {
	async fn tools(&self, _run: &Run) -> Result<Tools> {
		Ok(self.0.tools.clone())
	}
}

#[async_trait]
impl ExecutionEnvironment for Bench {
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
		Some(self)
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
			instructions: "Read the files".into(),
			knowledge_digest: None,
			tools: vec![],
			skills: vec![],
			max_steps: 64,
			allow_task_creation: None,
			conversation_memory: false,
			tool_parallelism: self.0.parallelism,
			projection_version: Default::default(),
		})
	}
	fn tool_slots(&self) -> Option<&ToolSlots> {
		self.0.slots.as_ref()
	}
	fn provider(&self, _model: ModelConfig) -> Result<Arc<dyn ModelProvider>> {
		unexpected("ExecutionEnvironment.provider")
	}
	fn compactor(&self) -> Result<Box<dyn CompactionClassifier>> {
		unexpected("ExecutionEnvironment.compactor")
	}
	fn binding_resolver(&self) -> &dyn BindingResolver {
		self
	}
	async fn documents(&self, _entry: &Entry) -> Result<Value> {
		unexpected("ExecutionEnvironment.documents")
	}
	async fn recheck_source_observation(&self, _run: &Run, _content: &Value) -> Result<()> {
		unexpected("ExecutionEnvironment.recheck_source_observation")
	}
	async fn skill_context(&self, _run: &Run) -> Result<String> {
		unexpected("ExecutionEnvironment.skill_context")
	}
	async fn skill_revision(&self, _run: &Run) -> Result<Option<i64>> {
		unexpected("ExecutionEnvironment.skill_revision")
	}
	async fn cache_scope(&self, _run: &Run) -> Result<aidash_domain::projection::CacheScope> {
		unexpected("ExecutionEnvironment.cache_scope")
	}
	async fn retrieval_scope(&self, _run: &Run) -> Result<RetrievalScope> {
		unexpected("ExecutionEnvironment.retrieval_scope")
	}
	async fn semantic_context(
		&self,
		_run: &Run,
		_task: &Task,
		_inputs: &[(InputRead, String)],
		_budget: usize,
		_entry: &Entry,
		_key: Option<&RetrievalKey>,
	) -> Result<Option<Value>> {
		unexpected("ExecutionEnvironment.semantic_context")
	}
	async fn run_message_limit(&self, _run: &Run) -> Result<usize> {
		Ok(16000)
	}
	async fn run_request_headroom(&self, _run: &Run) -> Result<usize> {
		Ok(16000)
	}
	async fn deliver_run_messages(&self, _run: &Run) -> Result<()> {
		Ok(())
	}
	async fn reconcile_run_messages(&self, _run: &Run) -> Result<()> {
		Ok(())
	}
	async fn require_terminal_safe_delivery(&self, _run: &Run) -> Result<()> {
		Ok(())
	}
	async fn transition_terminal_run_messages(
		&self,
		_run: &Run,
		_status: TaskStatus,
	) -> Result<()> {
		unexpected("ExecutionEnvironment.transition_terminal_run_messages")
	}
	async fn wait_for_inference_cancellation(&self, _run: Uuid) -> Result<()> {
		let mut cancelled = self.0.lab.cancel.subscribe();
		cancelled
			.wait_for(|cancelled| *cancelled)
			.await
			.map_err(|_| Error::Conflict("cancellation channel closed".into()))?;
		Ok(())
	}
	async fn operator_human_message_media(
		&self,
		_run: &Run,
		_messages: &[(i64, Uuid, usize)],
		_model: &ModelConfig,
	) -> Result<HumanMediaBatch> {
		unexpected("ExecutionEnvironment.operator_human_message_media")
	}
}

#[tokio::test]
async fn batched_reads_execute_concurrently_and_are_adopted_once() {
	// Arrange: each call blocks until its sibling is in flight too.
	let lab = Lab::rendezvous();
	let (bench, mut run) = bench(
		&lab,
		&["file_read", "file_search"],
		2,
		Some(ToolSlots::new(2)),
	);
	// Act
	advance(&bench, &mut run).await.unwrap();
	// Assert
	assert_eq!(lab.max_in_flight(), 2);
	assert_eq!(bench.admitted(), [keys(&run, 0..2)]);
	assert!(bench.starts().is_empty());
	assert_eq!(lab.events("dispatch"), keys(&run, 0..2));
	assert_eq!(adopted(&run), expected(&[(0, read("p0")), (1, read("p1"))]));
	assert_eq!(progress(&run), (2, None));
	assert_eq!(bench.saved(), ["run.tool_recorded"]);
	let saves = bench.0.saves.lock().unwrap();
	assert_eq!(progress(&saves[0].1), (2, None));
	assert_eq!(adopted(&saves[0].1), adopted(&run));
}

#[rstest]
#[case::run_ceiling_lower(2, Some(4), true)]
#[case::node_ceiling_lower(4, Some(2), true)]
#[case::node_ceiling_one(4, Some(1), false)]
#[case::node_without_slots(4, None, false)]
#[case::run_ceiling_one(1, Some(4), false)]
#[tokio::test]
async fn the_lower_of_run_and_node_ceilings_bounds_each_batch(
	#[case] parallelism: usize,
	#[case] slots: Option<usize>,
	#[case] batched: bool,
) {
	// Arrange
	let lab = Lab::lingering();
	let names = ["file_read", "file_read", "file_search"];
	let (bench, mut run) = bench(&lab, &names, parallelism, slots.map(ToolSlots::new));
	// Act
	advance(&bench, &mut run).await.unwrap();
	// Assert
	if batched {
		assert_eq!(bench.admitted(), [keys(&run, 0..2)]);
		assert!(bench.starts().is_empty());
		assert_eq!(lab.max_in_flight(), 2);
		assert_eq!(progress(&run), (2, None));
		// The remaining call is a batch of one: it takes the sequential path.
		advance(&bench, &mut run).await.unwrap();
		assert_eq!(bench.admitted().len(), 1);
		assert_eq!(bench.starts(), keys(&run, [2]));
		assert_eq!(lab.events("invoke"), keys(&run, [2]));
	} else {
		assert!(bench.admitted().is_empty());
		assert_eq!(bench.starts(), keys(&run, [0]));
		assert_eq!(lab.events("invoke"), keys(&run, [0]));
		assert!(lab.events("dispatch").is_empty());
		assert_eq!(lab.max_in_flight(), 1);
		assert_eq!(progress(&run), (1, None));
	}
	assert!(lab.max_in_flight() <= 2);
}

#[tokio::test]
async fn out_of_order_finishes_are_adopted_in_call_order() {
	// Arrange: call 0 finishes only after call 1 has finished.
	let gate = Arc::new(Notify::new());
	let lab = Lab::new(move |_, call| {
		let gate = gate.clone();
		async move {
			if call.path == "p0" {
				tokio::time::timeout(RENDEZVOUS, gate.notified())
					.await
					.map_err(|_| Error::External("call 1 never finished first".into()))?;
			} else {
				gate.notify_one();
			}
			Ok(read(&call.path))
		}
		.boxed()
	});
	let (bench, mut run) = bench(
		&lab,
		&["file_read", "file_read"],
		2,
		Some(ToolSlots::new(2)),
	);
	// Act
	advance(&bench, &mut run).await.unwrap();
	// Assert
	let run_id = run.id;
	assert_eq!(
		bench.admitted(),
		[vec![format!("{run_id}:7:0"), format!("{run_id}:7:1")]]
	);
	assert_eq!(bench.finished(), [key(&run, 1), key(&run, 0)]);
	assert_eq!(adopted(&run), expected(&[(0, read("p0")), (1, read("p1"))]));
	let calls = &run.state.tool().unwrap().response.tool_calls;
	let history_calls: Vec<_> = run
		.context
		.history
		.iter()
		.filter_map(|event| match event {
			ContextEvent::Tool { call, .. } => Some(call.clone()),
			_ => None,
		})
		.collect();
	assert_eq!(&history_calls, calls);
	assert_eq!(
		bench.reports(),
		[
			format!("{}:tool", key(&run, 0)),
			format!("{}:tool", key(&run, 1))
		]
	);
	assert_eq!(bench.status(&key(&run, 0)), ("COMPLETED", Some(read("p0"))));
	assert_eq!(bench.status(&key(&run, 1)), ("COMPLETED", Some(read("p1"))));
}

#[tokio::test]
async fn resumed_batch_reuses_completed_calls_and_dispatches_started_ones_once() {
	// Arrange: the worker stopped after call 0 finished and before call 1 did.
	let lab = Lab::echo();
	let (bench, mut run) = bench(
		&lab,
		&["file_read", "file_read"],
		2,
		Some(ToolSlots::new(2)),
	);
	run.state.tool_mut().unwrap().batch_end = Some(2);
	let journaled = json!({"content":"read before the crash"});
	bench.seed(&key(&run, 0), "COMPLETED", Some(journaled.clone()));
	bench.seed(&key(&run, 1), "STARTED", None);
	// Act
	advance(&bench, &mut run).await.unwrap();
	// Assert
	assert_eq!(lab.events("admit"), ["p1"]);
	assert_eq!(lab.events("dispatch"), keys(&run, [1]));
	assert_eq!(lab.attempts(&key(&run, 0)), 0);
	assert_eq!(bench.finished(), keys(&run, [1]));
	assert_eq!(adopted(&run), expected(&[(0, journaled), (1, read("p1"))]));
	assert_eq!(progress(&run), (2, None));
	assert_eq!(bench.saved(), ["run.tool_recorded"]);
}

#[tokio::test]
async fn admitted_batch_that_never_dispatched_resumes_each_call_once() {
	// Arrange: the worker stops after the admission transaction commits.
	let lab = Lab::echo();
	lab.admit_failures.store(1, SeqCst);
	let (bench, mut run) = bench(
		&lab,
		&["file_read", "file_search"],
		2,
		Some(ToolSlots::new(2)),
	);
	assert!(matches!(
		advance(&bench, &mut run).await,
		Err(Error::External(_))
	));
	assert!(lab.events("dispatch").is_empty());
	assert!(bench.saved().is_empty());
	assert_eq!(bench.status(&key(&run, 0)).0, "STARTED");
	assert_eq!(bench.status(&key(&run, 1)).0, "STARTED");
	let mut resumed = bench.admitted_run();
	assert_eq!(progress(&resumed), (0, Some(2)));
	// Act: a later worker resumes the persisted batch.
	advance(&bench, &mut resumed).await.unwrap();
	// Assert
	assert_eq!(bench.admitted(), [keys(&run, 0..2), keys(&run, 0..2)]);
	assert_eq!(lab.events("dispatch"), keys(&run, 0..2));
	assert!(bench.starts().is_empty());
	assert_eq!(
		adopted(&resumed),
		expected(&[(0, read("p0")), (1, read("p1"))])
	);
	assert_eq!(progress(&resumed), (2, None));
}

#[tokio::test]
async fn revoked_calls_of_a_resumed_batch_record_denials_without_dispatch() {
	// Arrange: call 0 completed and call 1 started before the restart; the
	// guard now denies both. Call 2 is still permitted.
	let lab = Lab::echo();
	let names = ["file_read", "file_read", "file_read"];
	let (bench, mut run) = bench(&lab, &names, 3, Some(ToolSlots::new(3)));
	run.state.tool_mut().unwrap().batch_end = Some(3);
	let journaled = json!({"content":"read before revocation"});
	bench.seed(&key(&run, 0), "COMPLETED", Some(journaled.clone()));
	bench.seed(&key(&run, 1), "STARTED", None);
	bench.seed(&key(&run, 2), "STARTED", None);
	bench.deny("call-0");
	bench.deny("call-1");
	// Act
	advance(&bench, &mut run).await.unwrap();
	// Assert
	assert_eq!(lab.events("admit"), ["p2"]);
	assert_eq!(lab.events("dispatch"), keys(&run, [2]));
	let denial = |index: usize| json!({"error": format!("call-{index} revoked")});
	assert_eq!(
		adopted(&run),
		expected(&[(0, denial(0)), (1, denial(1)), (2, read("p2"))])
	);
	// The completed row keeps its journaled result; the started row records the denial.
	assert_eq!(bench.status(&key(&run, 0)), ("COMPLETED", Some(journaled)));
	assert_eq!(bench.status(&key(&run, 1)), ("COMPLETED", Some(denial(1))));
	let mut finished = bench.finished();
	finished.sort();
	assert_eq!(finished, keys(&run, [1, 2]));
	assert_eq!(progress(&run), (3, None));
}

#[tokio::test]
async fn fresh_plan_denied_at_its_second_call_runs_the_first_call_sequentially() {
	// Arrange
	let lab = Lab::echo();
	let (bench, mut run) = bench(
		&lab,
		&["file_read", "file_read"],
		2,
		Some(ToolSlots::new(2)),
	);
	bench.deny("call-1");
	// Act
	advance(&bench, &mut run).await.unwrap();
	// Assert: nothing was admitted as a batch; call 0 used the sequential journal.
	assert!(bench.admitted().is_empty());
	assert_eq!(bench.starts(), keys(&run, [0]));
	assert_eq!(lab.events("invoke"), keys(&run, [0]));
	assert!(lab.events("dispatch").is_empty());
	assert_eq!(progress(&run), (1, None));
	assert!(
		bench
			.0
			.saves
			.lock()
			.unwrap()
			.iter()
			.all(|(_, saved)| progress(saved).1.is_none())
	);
	// The denied call is then recorded without a journal entry or execution.
	advance(&bench, &mut run).await.unwrap();
	assert_eq!(bench.starts(), keys(&run, [0]));
	assert_eq!(
		adopted(&run),
		expected(&[(0, read("p0")), (1, json!({"error":"call-1 revoked"}))])
	);
	assert_eq!(progress(&run), (2, None));
}

#[tokio::test]
async fn cancellation_during_a_batch_drops_calls_and_adopts_nothing() {
	// Arrange: the Run is cancelled once both calls are in flight; neither returns.
	let lab = Lab::new(|lab, _| {
		if lab.in_flight.load(SeqCst) == 2 {
			lab.cancel.send_replace(true);
		}
		std::future::pending::<Result<Value>>().boxed()
	});
	let (bench, mut run) = bench(
		&lab,
		&["file_read", "file_search"],
		2,
		Some(ToolSlots::new(2)),
	);
	// Act
	let result = advance(&bench, &mut run).await;
	// Assert
	assert!(matches!(result, Err(Error::Conflict(message)) if message.contains("cancelled")));
	assert_eq!(lab.events("dispatch"), keys(&run, 0..2));
	assert_eq!(lab.dropped.load(SeqCst), 2);
	assert_eq!(lab.in_flight.load(SeqCst), 0);
	assert!(bench.saved().is_empty());
	assert!(bench.finished().is_empty());
	assert!(bench.reports().is_empty());
	assert!(adopted(&run).is_empty());
	assert_eq!(bench.status(&key(&run, 0)).0, "STARTED");
	assert_eq!(bench.status(&key(&run, 1)).0, "STARTED");
}

#[tokio::test]
async fn infrastructure_error_lets_siblings_finish_and_reuses_them_later() {
	// Arrange: call 1 fails at once on its first attempt while call 0 is in flight.
	let lab = Lab::new(|_, call| {
		async move {
			match (call.path.as_str(), call.attempt) {
				("p1", 1) => Err(Error::External("disk unavailable".into())),
				("p0", _) => {
					tokio::time::sleep(Duration::from_millis(20)).await;
					Ok(read("p0"))
				}
				_ => Ok(read(&call.path)),
			}
		}
		.boxed()
	});
	let (bench, mut run) = bench(
		&lab,
		&["file_read", "file_read"],
		2,
		Some(ToolSlots::new(2)),
	);
	// Act
	let result = advance(&bench, &mut run).await;
	// Assert: the sibling finished and was journaled; nothing was adopted.
	assert!(matches!(result, Err(Error::External(message)) if message == "disk unavailable"));
	assert_eq!(bench.status(&key(&run, 0)), ("COMPLETED", Some(read("p0"))));
	assert_eq!(bench.status(&key(&run, 1)), ("STARTED", None));
	assert!(bench.saved().is_empty());
	assert!(bench.reports().is_empty());
	// Act: a later advance resumes the persisted batch.
	let mut resumed = bench.admitted_run();
	advance(&bench, &mut resumed).await.unwrap();
	// Assert
	assert_eq!(lab.attempts(&key(&run, 0)), 1);
	assert_eq!(lab.attempts(&key(&run, 1)), 2);
	assert_eq!(
		adopted(&resumed),
		expected(&[(0, read("p0")), (1, read("p1"))])
	);
	assert_eq!(progress(&resumed), (2, None));
}

#[tokio::test]
async fn concurrent_runs_share_the_node_ceiling_without_deadlock() {
	// Arrange: two workers on one Node, each with a batch of two; every call
	// holds its slot until some other call overlaps it and its result is recorded.
	let lab = Lab::rendezvous();
	let slots = ToolSlots::new(2);
	let (first, mut first_run) = bench(&lab, &["file_read", "file_read"], 2, Some(slots.clone()));
	let (second, mut second_run) = bench(&lab, &["file_search", "file_search"], 2, Some(slots));
	// Act
	let (first_result, second_result) = tokio::join!(
		advance(&first, &mut first_run),
		advance(&second, &mut second_run)
	);
	// Assert
	first_result.unwrap();
	second_result.unwrap();
	assert_eq!(lab.max_in_flight(), 2);
	assert_eq!(lab.events("dispatch").len(), 4);
	assert_eq!(first.admitted(), [keys(&first_run, 0..2)]);
	assert_eq!(second.admitted(), [keys(&second_run, 0..2)]);
	for run in [&first_run, &second_run] {
		assert_eq!(progress(run), (2, None));
		assert_eq!(adopted(run), expected(&[(0, read("p0")), (1, read("p1"))]));
	}
}

#[tokio::test]
async fn sequential_calls_between_reads_break_the_batch() {
	// Arrange: a write-capable call and a read pinned before SharedRead separate reads.
	let lab = Lab::echo();
	let names = [
		"file_read",
		"shell",
		"file_read",
		"file_search",
		"pinned_read",
		"file_read",
	];
	let (bench, mut run) = bench(&lab, &names, 4, Some(ToolSlots::new(4)));
	// Act
	for _ in 0..5 {
		advance(&bench, &mut run).await.unwrap();
	}
	// Assert
	assert_eq!(bench.admitted(), [keys(&run, 2..4)]);
	assert_eq!(bench.starts(), keys(&run, [0, 1, 4, 5]));
	assert_eq!(lab.events("dispatch"), keys(&run, 2..4));
	assert_eq!(lab.events("invoke"), keys(&run, [0, 1, 4, 5]));
	assert_eq!(progress(&run), (6, None));
	let results: Vec<_> = (0..6)
		.map(|index| (index, read(&format!("p{index}"))))
		.collect();
	assert_eq!(adopted(&run), expected(&results));
}
