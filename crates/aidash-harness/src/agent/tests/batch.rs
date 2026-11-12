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
	/// The next N admissions are refused with a sentence for the model.
	admit_refusals: AtomicUsize,
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
			admit_refusals: AtomicUsize::new(0),
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
		if self
			.lab
			.admit_refusals
			.fetch_update(SeqCst, SeqCst, |left| left.checked_sub(1))
			.is_ok()
		{
			return Err(Error::Refused("admission refused the call".into()));
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
		"unsafe_test" => (builtin_contract("live_preview_start").unwrap(), None),
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
	delivery_failures: u32,
	delivery_pending: bool,
	uncertain_reason: Option<String>,
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
	/// Calls naming a definition the Registry lacks: the gate answers `NotFound` (#813).
	missing: Mutex<BTreeSet<String>>,
	/// Calls the gate refuses with an Authority Pause (the Tenant policy denies them).
	refused: Mutex<BTreeSet<String>>,
	/// Every call the gate was asked about.
	consulted: Mutex<Vec<String>>,
	/// Run messages accepted so far, newest last.
	inputs: Mutex<Vec<aidash_domain::run_input::RunInput>>,
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
	fn miss(&self, call: &str) {
		self.0.missing.lock().unwrap().insert(call.into());
	}
	/// A person's Run message the Run accepted after its response was stored.
	fn accept_message(&self, seq: i64, content: &str) {
		self.0
			.inputs
			.lock()
			.unwrap()
			.push(aidash_domain::run_input::RunInput {
				seq,
				sender: "alice".into(),
				content: content.into(),
				idempotency_key: format!("human:{seq}"),
				message_id: None,
				reference_only: false,
			});
	}
	fn refuse(&self, call: &str) {
		self.0.refused.lock().unwrap().insert(call.into());
	}
	fn consulted(&self) -> Vec<String> {
		self.0.consulted.lock().unwrap().clone()
	}
	fn seed(&self, key: &str, status: &'static str, result: Option<Value>) {
		self.0.journal.lock().unwrap().insert(
			key.into(),
			Invocation {
				name: "file_read".into(),
				status,
				result,
				replay_safe: true,
				delivery_failures: 0,
				delivery_pending: false,
				uncertain_reason: None,
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
		node: "brave-otter-k3m9q2xa".parse().unwrap(),
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
		control_revision: 0,
		control_source: Default::default(),
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
		missing: Mutex::new(BTreeSet::new()),
		refused: Mutex::new(BTreeSet::new()),
		consulted: Mutex::new(vec![]),
		inputs: Mutex::new(vec![]),
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
		.events()
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
	async fn unfinished_owned_tasks(&self, _: &Run) -> Result<u64> {
		Ok(0)
	}
	async fn workspace_events(&self, _: &Run, _: i64) -> Result<WorkspaceEvents> {
		Ok(WorkspaceEvents::default())
	}
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
	async fn start_inference(
		&self,
		_run: &Run,
		_token: Uuid,
		_attempt: InferenceAttemptId,
	) -> Result<()> {
		unexpected("ExecutionStore.start_inference")
	}
	async fn append_inference_progress(
		&self,
		_run: &Run,
		_token: Uuid,
		_attempt: InferenceAttemptId,
		_batch: &[InferenceProgress],
	) -> Result<()> {
		unexpected("ExecutionStore.append_inference_progress")
	}
	async fn finish_inference(
		&self,
		_run: &Run,
		_token: Uuid,
		_attempt: InferenceAttemptId,
		_outcome: ProgressOutcome,
	) -> Result<()> {
		unexpected("ExecutionStore.finish_inference")
	}
	async fn run_inputs(&self, _run: Uuid) -> Result<Vec<aidash_domain::run_input::RunInput>> {
		Ok(self.0.inputs.lock().unwrap().clone())
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
		if let Some(existing) = journal.get_mut(key) {
			assert_eq!(existing.name, name);
			if existing.status == "REFUSED" {
				existing.status = "STARTED";
				existing.result = None;
			}
			let retrying = std::mem::take(&mut existing.delivery_pending);
			let status = if existing.status == "STARTED" && !existing.replay_safe && !retrying {
				"UNCERTAIN"
			} else {
				existing.status
			};
			return Ok(InvocationOutcome {
				status: status.into(),
				result: existing.result.clone(),
				uncertain_reason: existing.uncertain_reason.clone(),
				retrying,
			});
		}
		journal.insert(
			key.into(),
			Invocation {
				name: name.into(),
				status: "STARTED",
				result: None,
				replay_safe,
				delivery_failures: 0,
				delivery_pending: false,
				uncertain_reason: None,
			},
		);
		Ok(InvocationOutcome {
			status: "STARTED".into(),
			result: None,
			uncertain_reason: None,
			retrying: false,
		})
	}
	async fn invocation_not_delivered(
		&self,
		_run: &Run,
		token: Uuid,
		key: &str,
		failure: &Value,
	) -> Result<u32> {
		assert_eq!(token, self.0.token);
		let mut journal = self.0.journal.lock().unwrap();
		let entry = journal.get_mut(key).unwrap();
		entry.delivery_failures += 1;
		entry.delivery_pending = true;
		if entry.delivery_failures > aidash_domain::semantic::OUTAGE_RETRIES {
			entry.status = "COMPLETED";
			entry.result = Some(failure.clone());
		}
		Ok(entry.delivery_failures)
	}
	async fn invocation_refused(
		&self,
		_run: &Run,
		token: Uuid,
		key: &str,
		refusal: &aidash_domain::tool::AuthorityRefusal,
	) -> Result<()> {
		assert_eq!(token, self.0.token);
		let mut journal = self.0.journal.lock().unwrap();
		let entry = journal.get_mut(key).unwrap();
		if entry.status == "STARTED" {
			entry.status = "REFUSED";
			entry.result =
				Some(json!({"error":{"kind":"authority_refused","message":refusal.to_string()}}));
		}
		Ok(())
	}
	async fn invocation_answer_lost(&self, _run: &Run, token: Uuid, key: &str) -> Result<()> {
		assert_eq!(token, self.0.token);
		self.0
			.journal
			.lock()
			.unwrap()
			.get_mut(key)
			.unwrap()
			.uncertain_reason = Some("answer_lost".into());
		Ok(())
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
					delivery_failures: 0,
					delivery_pending: false,
					uncertain_reason: None,
				});
				assert_eq!(invocation.name, call.name);
				if invocation.status == "REFUSED" {
					invocation.status = "STARTED";
					invocation.result = None;
				}
				InvocationOutcome {
					status: invocation.status.into(),
					result: invocation.result.clone(),
					uncertain_reason: invocation.uncertain_reason.clone(),
					retrying: std::mem::take(&mut invocation.delivery_pending),
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
	) -> Result<Value> {
		assert_eq!(token, self.0.token);
		let mut flight = Flight::enter(&self.0.lab);
		{
			let mut journal = self.0.journal.lock().unwrap();
			let invocation = journal
				.get_mut(key)
				.expect("finished invocation was started");
			if invocation.status == "COMPLETED" {
				assert_eq!(invocation.result.as_ref(), Some(output));
			} else {
				assert_eq!(invocation.status, "STARTED");
				invocation.status = "COMPLETED";
				invocation.result = Some(output.clone());
				self.0
					.finishes
					.lock()
					.unwrap()
					.push((key.into(), output.clone()));
			}
		}
		// The recording connection stays busy after the row is written.
		tokio::time::sleep(Duration::from_millis(10)).await;
		flight.returned = true;
		Ok(output.clone())
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
	async fn record_usage_dispatch(
		&self,
		_run: &Run,
		_token: Uuid,
		_dispatch: &UsageDispatch,
	) -> Result<()> {
		unexpected("ExecutionStore.record_usage_dispatch")
	}
	async fn complete_usage_record(
		&self,
		_run: &Run,
		_attempt: Uuid,
		_outcome: &UsageOutcome,
	) -> Result<()> {
		unexpected("ExecutionStore.complete_usage_record")
	}
	async fn run_message_has_media(&self, _messages: &[Uuid]) -> Result<bool> {
		Ok(false)
	}
	async fn context_journal(
		&self,
		_run: Uuid,
		_from: u64,
		_through: u64,
	) -> Result<Vec<aidash_domain::context::HistoryEntry>> {
		unexpected("ExecutionStore.context_journal")
	}
	async fn journal_context(&self, _run: &Run, _token: Uuid) -> Result<()> {
		unexpected("ExecutionStore.journal_context")
	}
	async fn begin_compaction(
		&self,
		_run: &Run,
		_token: Uuid,
		_attempt: &aidash_domain::context::recovery::Attempt,
		_call_budget: u32,
	) -> Result<()> {
		unexpected("ExecutionStore.begin_compaction")
	}
	async fn settle_compaction(
		&self,
		_run: &Run,
		_token: Uuid,
		_attempt: Uuid,
		_settlement: &aidash_domain::context::recovery::Settlement,
	) -> Result<()> {
		unexpected("ExecutionStore.settle_compaction")
	}
	async fn adopt_compaction(
		&self,
		_run: &Run,
		_token: Uuid,
		_attempt: Uuid,
		_settlement: &aidash_domain::context::recovery::Settlement,
		_staged: Option<&aidash_application::decision::StagedDecision>,
	) -> Result<()> {
		unexpected("ExecutionStore.adopt_compaction")
	}
}

#[async_trait]
impl ExecutionCatalog for Bench {
	async fn get_for_run(&self, _run: &Run, id: &str, _version: &str) -> Result<Entry> {
		Ok(serde_json::from_value(
			json!({"id":id,"version":"1.0.0","kind":"agent","name":{"en":id},"description":{"en":"Fixture"},"config":{
				"provider":"openrouter","model_id":"fixture","endpoint":"http://fixture.invalid/v1","context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}
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
	async fn complete(
		&self,
		_key: &str,
		_artifact: &ArtifactInput,
		_abandon: &[Uuid],
	) -> Result<Task> {
		unexpected("ExecutionHome.complete")
	}
	async fn abandon_unassigned(&self, _parent: Uuid, _ids: &[Uuid]) -> Result<()> {
		unexpected("ExecutionHome.abandon_unassigned")
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
		self.0.consulted.lock().unwrap().push(call.id.clone());
		if self.0.refused.lock().unwrap().contains(&call.id) {
			return Err(Error::AuthorityRefused(Box::new(
				aidash_domain::tool::AuthorityRefusal {
					tool: call.name.clone(),
					check: aidash_domain::tool::RefusalCheck::Policy,
					action: Some("task.delegate".into()),
					subject: None,
					call: None,
				},
			)));
		}
		if self.0.denied.lock().unwrap().contains(&call.id) {
			return Err(Error::Invalid(format!("{} revoked", call.id)));
		}
		if self.0.missing.lock().unwrap().contains(&call.id) {
			return Err(Error::NotFound(format!("{} names no definition", call.id)));
		}
		Ok(())
	}
	async fn human_read(&self, _id: Uuid) -> Result<()> {
		unexpected("ExecutionAuthority.human_read")
	}
	async fn tool_call_approval(
		&self,
		_run: &Run,
		_request: ToolCallRequest,
	) -> Result<ToolCallApproval> {
		unexpected("ExecutionAuthority.tool_call_approval")
	}
	async fn approval_mode(
		&self,
		_run: &Run,
	) -> Result<Option<aidash_domain::approval_rules::ApprovalMode>> {
		unexpected("ExecutionAuthority.approval_mode")
	}
	async fn answer_deadline(&self, _run: &Run) -> Result<Option<i64>> {
		unexpected("ExecutionAuthority.answer_deadline")
	}
	async fn approval_judgement(
		&self,
		_run: &Run,
		_approval: Uuid,
	) -> Result<Option<aidash_application::capabilities::approvals::JudgeRequest>> {
		unexpected("ExecutionAuthority.approval_judgement")
	}
	async fn settle_judgement(
		&self,
		_run: &Run,
		_approval: Uuid,
		_settlement: aidash_application::capabilities::approvals::Settlement,
	) -> Result<bool> {
		unexpected("ExecutionAuthority.settle_judgement")
	}
	async fn model_media(
		&self,
		_selections: &[aidash_domain::media::ModelMedia],
	) -> Result<Vec<ContentPart>> {
		unexpected("ExecutionAuthority.model_media")
	}
	async fn human_message_media(
		&self,
		_messages: &[(i64, Uuid, usize)],
		_model: &ModelConfig,
		_projection: aidash_domain::projection::ProjectionVersion,
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
	async fn recheck_inference(&self) -> Result<()> {
		unexpected("ExecutionAuthority.recheck_inference")
	}
	async fn resume(&self) -> Result<()> {
		unexpected("ExecutionAuthority.resume")
	}
	async fn reserve_summary(
		&self,
		_token: Uuid,
		_summarizer: &aidash_domain::context::summary::SummaryProvider,
		_window: usize,
		_output: u32,
		_request: &ModelRequest,
	) -> Result<Option<Box<dyn InferenceReservation>>> {
		unexpected("ExecutionAuthority.reserve_summary")
	}
	async fn recheck_summary(
		&self,
		_summarizer: &aidash_domain::context::summary::SummaryProvider,
	) -> Result<()> {
		unexpected("ExecutionAuthority.recheck_summary")
	}
	async fn recheck_decider(&self, _: &aidash_domain::decision::DeciderPin) -> Result<()> {
		unexpected("ExecutionAuthority.recheck_decider")
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
	fn context_source_plan(&self, _run: &Run) -> Result<Vec<PlannedContextSource>> {
		Ok(super::fixture_source_plan(false, false, false))
	}
	async fn context_source(
		&self,
		run: &Run,
		source: &ContextSourceHandle,
		_worker: Uuid,
		frame: ContextSourceFrame<'_>,
	) -> SemanticContextRead {
		let result = match source.as_str() {
			"workspace" => self
				.home(run)
				.observation(0, context::observation::DEFAULT_LIMIT)
				.await
				.map(Some),
			"semantic_memory" => {
				return self.semantic_context(frame).await;
			}
			_ => unexpected("unknown planned context source"),
		};
		SemanticContextRead {
			selections: vec![],
			contributions: vec![],
			workspace_retrieved: false,
			result,
		}
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
		Some(self)
	}
	fn home(&self, _run: &Run) -> Box<dyn ExecutionHome> {
		Box::new(self.clone())
	}
	fn agent(&self, _entry: &Entry) -> Result<ExecutionAgent> {
		Ok(ExecutionAgent {
			reference_files: 0,
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
			context_policy: None,
			tool_parallelism: self.0.parallelism,
			projection_version: Default::default(),
			harness_contract: Default::default(),
			prompt_cache: Default::default(),
			completion: None,
		})
	}
	fn tool_slots(&self) -> Option<&ToolSlots> {
		self.0.slots.as_ref()
	}
	fn provider(&self, _model: ModelConfig) -> Result<Arc<dyn ModelProvider>> {
		unexpected("ExecutionEnvironment.provider")
	}
	fn binding_resolver(&self) -> &dyn BindingResolver {
		self
	}

	async fn recheck_source_observation(&self, _run: &Run, _content: &Value) -> Result<()> {
		unexpected("ExecutionEnvironment.recheck_source_observation")
	}

	async fn direct_skills(&self, _run: &Run) -> Result<Vec<aidash_domain::exposure::DirectSkill>> {
		Ok(vec![])
	}
	async fn direct_skill_body(
		&self,
		_run: &Run,
		_skill_id: Uuid,
		_digest: &str,
	) -> Result<Option<String>> {
		unexpected("ExecutionEnvironment.direct_skill_body")
	}
	async fn source_revision(
		&self,
		_source: &ContextSourceHandle,
		_run: &Run,
	) -> Result<Option<i64>> {
		unexpected("ExecutionEnvironment.skill_revision")
	}
	async fn cache_scope(&self, _run: &Run) -> Result<aidash_domain::projection::CacheScope> {
		unexpected("ExecutionEnvironment.cache_scope")
	}
	async fn retrieval_scope(&self, _run: &Run) -> Result<RetrievalScope> {
		unexpected("ExecutionEnvironment.retrieval_scope")
	}
	async fn source_retrieval_key(
		&self,
		_run: &Run,
		_bindings: &str,
		_frame: ContextSourceFrame<'_>,
	) -> Result<RetrievalKey> {
		unexpected("ExecutionEnvironment.source_retrieval_key")
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
	async fn wait_for_inference_interruption(
		&self,
		_run: Uuid,
		included_input_seq: i64,
	) -> Result<InferenceInterruption> {
		// A Tool Batch waits only for cancellation; no input supersedes it.
		assert_eq!(included_input_seq, i64::MAX);
		let mut cancelled = self.0.lab.cancel.subscribe();
		cancelled
			.wait_for(|cancelled| *cancelled)
			.await
			.map_err(|_| Error::Conflict("cancellation channel closed".into()))?;
		Ok(InferenceInterruption::Cancelled)
	}
	async fn summary_dependencies_current(
		&self,
		_run: &Run,
		_dependencies: &aidash_domain::context::summary::SummaryDependencies,
	) -> Result<bool> {
		unexpected("ExecutionEnvironment.summary_dependencies_current")
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
		.events()
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

/// A missing definition is a call error at the batch gate, not an Authority Pause (#813).
#[tokio::test]
async fn fresh_plan_naming_a_missing_definition_runs_the_first_call_and_answers_the_second() {
	// Arrange
	let lab = Lab::echo();
	let (bench, mut run) = bench(
		&lab,
		&["file_read", "file_read"],
		2,
		Some(ToolSlots::new(2)),
	);
	bench.miss("call-1");
	// Act
	advance(&bench, &mut run).await.unwrap();
	// Assert: the batch stops before the missing call; call 0 runs sequentially.
	assert!(bench.admitted().is_empty());
	assert_eq!(bench.starts(), keys(&run, [0]));
	assert_eq!(progress(&run), (1, None));
	// The missing call is then answered with the error, without a journal entry.
	advance(&bench, &mut run).await.unwrap();
	assert_eq!(bench.starts(), keys(&run, [0]));
	assert_eq!(
		adopted(&run),
		expected(&[
			(0, read("p0")),
			(1, json!({"error":"call-1 names no definition"}))
		])
	);
	assert_eq!(progress(&run), (2, None));
}

/// A resumed batch whose definition went missing records the call error for that call (#813).
#[tokio::test]
async fn a_missing_definition_in_a_resumed_batch_records_a_call_error_without_dispatch() {
	// Arrange: both calls started before the restart; call 0's definition is now missing.
	let lab = Lab::echo();
	let (bench, mut run) = bench(
		&lab,
		&["file_read", "file_read"],
		2,
		Some(ToolSlots::new(2)),
	);
	run.state.tool_mut().unwrap().batch_end = Some(2);
	bench.seed(&key(&run, 0), "STARTED", None);
	bench.seed(&key(&run, 1), "STARTED", None);
	bench.miss("call-0");
	// Act
	advance(&bench, &mut run).await.unwrap();
	// Assert
	assert_eq!(lab.events("admit"), ["p1"]);
	assert_eq!(lab.events("dispatch"), keys(&run, [1]));
	let missing = json!({"error":"call-0 names no definition"});
	assert_eq!(
		adopted(&run),
		expected(&[(0, missing.clone()), (1, read("p1"))])
	);
	assert_eq!(bench.status(&key(&run, 0)), ("COMPLETED", Some(missing)));
	assert_eq!(progress(&run), (2, None));
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
async fn an_unsafe_call_is_reexecuted_after_a_not_delivered_attempt() {
	let lab = Lab::new(|_, call| {
		async move {
			if call.attempt == 1 {
				Err(Error::ToolNotDelivered {
					message: "connection refused".into(),
					retry_after: None,
				})
			} else {
				Ok(read(&call.path))
			}
		}
		.boxed()
	});
	let (bench, mut run) = bench(&lab, &["unsafe_test"], 1, None);
	assert!(matches!(
		advance(&bench, &mut run).await,
		Err(Error::ToolRetry { count: 1, .. })
	));
	advance(&bench, &mut run).await.unwrap();
	assert_eq!(lab.attempts(&key(&run, 0)), 2);
	assert_eq!(adopted(&run), expected(&[(0, read("p0"))]));
}

#[tokio::test]
async fn sequential_tool_error_is_recorded_for_the_model() {
	let expected_error = json!({"error":{"kind":"http_status","status":422,"body":"invalid"}});
	let output = expected_error.clone();
	let lab = Lab::new(move |_, _| {
		let output = output.clone();
		async move { Ok(output) }.boxed()
	});
	let (bench, mut run) = bench(&lab, &["file_read"], 1, None);
	advance(&bench, &mut run).await.unwrap();
	assert_eq!(adopted(&run), expected(&[(0, expected_error)]));
	assert_eq!(bench.status(&key(&run, 0)).0, "COMPLETED");
}

#[tokio::test]
async fn a_batch_retries_not_delivered_calls_then_records_exhaustion() {
	let lab = Lab::new(|_, call| {
		async move {
			if call.path == "p1" {
				Err(Error::ToolNotDelivered {
					message: "connection refused".into(),
					retry_after: None,
				})
			} else {
				Ok(read(&call.path))
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
	for count in 1..=aidash_domain::semantic::OUTAGE_RETRIES {
		let result = advance(&bench, &mut run).await;
		assert!(matches!(result, Err(Error::ToolRetry { count: actual, .. }) if actual == count));
		run = bench.admitted_run();
	}
	advance(&bench, &mut run).await.unwrap();
	assert_eq!(lab.attempts(&key(&run, 0)), 1);
	assert_eq!(lab.attempts(&key(&run, 1)), 6);
	assert_eq!(
		adopted(&run),
		expected(&[
			(0, read("p0")),
			(
				1,
				json!({"error":{"kind":"not_delivered","message":"connection refused"}})
			)
		])
	);
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

/// Bind `aliases` as Registry tools of a `deferred@1` Agent; `eager` ones are
/// in every request's Exposure set, the rest only after a Load.
fn deferred(run: &mut Run, aliases: &[&str], eager: &[&str]) {
	let mut snapshot = DeferredSnapshot::default();
	for alias in aliases {
		snapshot.bind(
			reference(alias),
			"tool",
			&format!("Fixture {alias}."),
			json!(core_descriptor(NODE, alias).unwrap()),
			Some(alias),
			BindingOrigin::Explicit,
		);
	}
	for alias in eager {
		snapshot.eager("tool", alias);
	}
	run.context.binding_snapshot = Some(Box::new(snapshot.snapshot()));
}

#[tokio::test]
async fn reads_loaded_in_this_response_wait_for_the_next_request() {
	// Arrange: a Load earlier in this response staged file_search for the next
	// request; the request that produced these calls did not advertise it.
	let lab = Lab::echo();
	let names = ["file_search", "file_search"];
	let (bench, mut run) = bench(&lab, &names, 4, Some(ToolSlots::new(4)));
	deferred(&mut run, &["file_search"], &[]);
	let digest = run.context.binding_snapshot.as_ref().unwrap().bindings[0]
		.digest
		.clone();
	run.context.exposure.stage(ExposureUpdate::Load(Loaded {
		alias: "file_search".into(),
		kind: CapabilityKind::Tool,
		identity: exposure::CapabilityIdentity::Registry(reference("file_search")),
		digest,
		step: run.step,
	}));
	// Act
	advance(&bench, &mut run).await.unwrap();
	advance(&bench, &mut run).await.unwrap();
	// Assert: each call records the sequential path's denial; nothing runs.
	let denial = json!({"error": "capability file_search was loaded in this response; call it after the next model request"});
	assert!(bench.admitted().is_empty());
	assert!(bench.starts().is_empty());
	assert!(lab.events("dispatch").is_empty());
	assert!(lab.events("invoke").is_empty());
	assert_eq!(adopted(&run), expected(&[(0, denial.clone()), (1, denial)]));
	assert_eq!(progress(&run), (2, None));
}

#[tokio::test]
async fn unloaded_aliases_end_the_batch_and_record_the_exposure_denial() {
	// Arrange: file_read is eager; file_search was never loaded.
	let lab = Lab::echo();
	let names = ["file_read", "file_read", "file_search", "file_search"];
	let (bench, mut run) = bench(&lab, &names, 4, Some(ToolSlots::new(4)));
	deferred(&mut run, &["file_read", "file_search"], &["file_read"]);
	// Act
	for _ in 0..3 {
		advance(&bench, &mut run).await.unwrap();
	}
	// Assert: only the exposed reads form a batch.
	let denial = json!({"error": "capability file_search is not loaded; use capability_load"});
	assert_eq!(bench.admitted(), [keys(&run, 0..2)]);
	assert!(bench.starts().is_empty());
	assert_eq!(lab.events("dispatch"), keys(&run, 0..2));
	assert!(lab.events("invoke").is_empty());
	assert_eq!(
		adopted(&run),
		expected(&[
			(0, read("p0")),
			(1, read("p1")),
			(2, denial.clone()),
			(3, denial),
		])
	);
	assert_eq!(progress(&run), (4, None));
}

impl Bench {
	async fn semantic_context(&self, _frame: ContextSourceFrame<'_>) -> SemanticContextRead {
		unexpected("ExecutionEnvironment.semantic_context")
	}
}

/// A check that refused the call and left a note, as `Access::require` does.
fn refused_by_policy(action: &str) -> Error {
	refusal_note::set(refusal_note::Note::new(
		aidash_domain::tool::RefusalCheck::Policy,
		Some(action),
	));
	Error::Forbidden
}

#[tokio::test]
async fn an_unclassified_refusal_reaches_the_model_and_the_run_goes_on() {
	// Arrange
	let lab = Lab::new(|_, _| async { Err(Error::Forbidden) }.boxed());
	let (bench, mut run) = bench(&lab, &["file_read"], 1, None);
	// Act
	advance(&bench, &mut run).await.unwrap();
	// Assert
	let results = adopted(&run);
	let message = results[0].1["error"].as_str().unwrap();
	assert!(
		message.contains("An internal check refused this call"),
		"{message}"
	);
	assert!(message.contains("Do not repeat this call"), "{message}");
	assert!(!message.contains("partly happened"), "{message}");
	assert_eq!(bench.status(&key(&run, 0)).0, "COMPLETED");
}

#[tokio::test]
async fn an_unclassified_refusal_of_an_unsafe_tool_says_it_may_have_partly_happened() {
	let lab = Lab::new(|_, _| async { Err(Error::Unauthorized) }.boxed());
	let (bench, mut run) = bench(&lab, &["unsafe_test"], 1, None);
	advance(&bench, &mut run).await.unwrap();
	let message = adopted(&run)[0].1["error"].as_str().unwrap().to_owned();
	assert!(message.contains("may have partly happened"), "{message}");
	assert_eq!(bench.status(&key(&run, 0)).0, "COMPLETED");
}

#[tokio::test]
async fn not_found_is_a_call_error_even_after_a_swallowed_denial() {
	// Arrange: a denial was ignored, then the tool ended with a plain NotFound.
	let lab = Lab::new(|_, _| {
		async {
			refusal_note::set(refusal_note::Note::new(
				aidash_domain::tool::RefusalCheck::Policy,
				Some("reference.read"),
			));
			Err(Error::NotFound("file unavailable".into()))
		}
		.boxed()
	});
	let (bench, mut run) = bench(&lab, &["file_read"], 1, None);
	// Act
	advance(&bench, &mut run).await.unwrap();
	// Assert
	let message = adopted(&run)[0].1["error"].as_str().unwrap().to_owned();
	assert!(message.starts_with("file unavailable. "), "{message}");
	assert_eq!(bench.status(&key(&run, 0)).0, "COMPLETED");
}

#[tokio::test]
async fn not_found_from_an_unsafe_tool_says_it_may_have_partly_happened() {
	let lab = Lab::new(|_, _| async { Err(Error::NotFound("preview unavailable".into())) }.boxed());
	let (bench, mut run) = bench(&lab, &["unsafe_test"], 1, None);
	advance(&bench, &mut run).await.unwrap();
	let message = adopted(&run)[0].1["error"].as_str().unwrap().to_owned();
	assert_eq!(
		message,
		"preview unavailable. The call may have partly happened. Do not repeat this call."
	);
}

/// The fields of every event logged since `capture_logs` first ran. The subscriber is global
/// because a thread-local one misses events when parallel tests register the same call site first:
/// tracing then caches it as disabled for threads that set their own subscriber later.
static LOGGED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
struct LoggedEvents;
impl tracing::Subscriber for LoggedEvents {
	fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
		true
	}
	fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
		tracing::span::Id::from_u64(1)
	}
	fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
	fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
	fn event(&self, event: &tracing::Event<'_>) {
		struct Fields(String);
		impl tracing::field::Visit for Fields {
			fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
				use std::fmt::Write;
				let _ = write!(self.0, "{}={value:?} ", field.name());
			}
		}
		let mut fields = Fields(String::new());
		event.record(&mut fields);
		LOGGED.lock().unwrap().push(fields.0);
	}
	fn enter(&self, _: &tracing::span::Id) {}
	fn exit(&self, _: &tracing::span::Id) {}
}
fn capture_logs() {
	static INSTALL: std::sync::Once = std::sync::Once::new();
	INSTALL.call_once(|| {
		tracing::subscriber::set_global_default(LoggedEvents)
			.expect("no other test sets a global subscriber");
	});
}

#[tokio::test]
async fn a_not_found_call_leaves_a_log_line_naming_the_run_and_the_tool() {
	// Arrange: a refusal without a policy decision leaves no row, so the log is its record.
	capture_logs();
	let lab = Lab::new(|_, _| async { Err(Error::NotFound("file unavailable".into())) }.boxed());
	let (bench, mut run) = bench(&lab, &["file_read"], 1, None);
	// Act
	advance(&bench, &mut run).await.unwrap();
	// Assert: other tests log too, so only this Run's lines count.
	let lines = LOGGED.lock().unwrap().clone();
	assert!(
		lines.iter().any(|line| {
			line.contains(&run.id.to_string())
				&& line.contains("file_read")
				&& line.contains("file unavailable")
		}),
		"{lines:?}"
	);
}

#[tokio::test]
async fn a_conflict_still_fails_the_run() {
	let lab = Lab::new(|_, _| {
		async {
			Err(Error::Conflict(
				"admitted Binding definition changed".into(),
			))
		}
		.boxed()
	});
	let (bench, mut run) = bench(&lab, &["file_read"], 1, None);
	let result = advance(&bench, &mut run).await;
	assert!(matches!(result, Err(Error::Conflict(_))));
	assert_eq!(bench.status(&key(&run, 0)).0, "STARTED");
}

#[tokio::test]
async fn a_noted_refusal_pauses_naming_the_check_and_refuses_the_invocation() {
	let lab = Lab::new(|_, _| async { Err(refused_by_policy("task.delegate")) }.boxed());
	let (bench, mut run) = bench(&lab, &["file_read"], 1, None);
	let result = advance(&bench, &mut run).await;
	let Err(Error::AuthorityRefused(refusal)) = result else {
		panic!("expected a named pause, got {result:?}");
	};
	assert_eq!(refusal.tool, "file_read");
	assert_eq!(refusal.check, aidash_domain::tool::RefusalCheck::Policy);
	assert_eq!(refusal.action.as_deref(), Some("task.delegate"));
	assert_eq!(bench.status(&key(&run, 0)).0, "REFUSED");
}

#[tokio::test]
async fn resuming_a_refused_invocation_runs_the_call_again() {
	let lab = Lab::new(|_, call| {
		async move {
			if call.attempt == 1 {
				Err(refused_by_policy("task.delegate"))
			} else {
				Ok(read(&call.path))
			}
		}
		.boxed()
	});
	let (bench, mut run) = bench(&lab, &["file_read"], 1, None);
	assert!(advance(&bench, &mut run).await.is_err());
	assert_eq!(bench.status(&key(&run, 0)).0, "REFUSED");
	advance(&bench, &mut run).await.unwrap();
	assert_eq!(adopted(&run), expected(&[(0, read("p0"))]));
	assert_eq!(bench.status(&key(&run, 0)).0, "COMPLETED");
}

#[tokio::test]
async fn a_noted_refusal_of_an_unsafe_tool_stays_started() {
	let lab = Lab::new(|_, _| async { Err(refused_by_policy("preview.start")) }.boxed());
	let (bench, mut run) = bench(&lab, &["unsafe_test"], 1, None);
	let result = advance(&bench, &mut run).await;
	assert!(matches!(result, Err(Error::AuthorityRefused(_))));
	assert_eq!(bench.status(&key(&run, 0)).0, "STARTED");
}

#[tokio::test]
async fn a_run_authority_refusal_after_the_effect_leaves_the_invocation_started() {
	let lab = Lab::new(|_, _| {
		async {
			Err(Error::AuthorityRefused(Box::new(
				aidash_domain::tool::AuthorityRefusal {
					tool: "file_read".into(),
					check: aidash_domain::tool::RefusalCheck::RunAuthority,
					action: None,
					subject: None,
					call: None,
				},
			)))
		}
		.boxed()
	});
	let (bench, mut run) = bench(&lab, &["file_read"], 1, None);
	let result = advance(&bench, &mut run).await;
	assert!(matches!(result, Err(Error::AuthorityRefused(_))));
	assert_eq!(bench.status(&key(&run, 0)).0, "STARTED");
}

#[tokio::test]
async fn a_refused_error_from_a_batched_call_is_a_call_error() {
	let lab = Lab::new(|_, call| {
		async move {
			if call.path == "p1" {
				Err(Error::Refused(
					"task t was not found or is not visible to this Run".into(),
				))
			} else {
				Ok(read(&call.path))
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
	advance(&bench, &mut run).await.unwrap();
	assert_eq!(
		adopted(&run),
		expected(&[
			(0, read("p0")),
			(
				1,
				json!({"error":"task t was not found or is not visible to this Run"})
			)
		])
	);
}

#[tokio::test]
async fn batched_calls_do_not_see_each_others_notes() {
	// Arrange: call 0 leaves a note and ends with NotFound; call 1 ends with a bare Forbidden.
	let lab = Lab::new(|_, call| {
		async move {
			if call.path == "p0" {
				refusal_note::set(refusal_note::Note::new(
					aidash_domain::tool::RefusalCheck::Policy,
					Some("reference.read"),
				));
				tokio::task::yield_now().await;
				Err(Error::NotFound("gone".into()))
			} else {
				tokio::task::yield_now().await;
				Err(Error::Forbidden)
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
	advance(&bench, &mut run).await.unwrap();
	// Assert: call 1 saw no note, so it is an unclassified Rejected Call, not a pause.
	let results = adopted(&run);
	assert!(
		results[1].1["error"]
			.as_str()
			.unwrap()
			.contains("An internal check refused this call")
	);
}

#[tokio::test]
async fn a_refused_admission_is_a_call_error_and_the_batch_goes_on() {
	let lab = Lab::new(|_, call| async move { Ok(read(&call.path)) }.boxed());
	lab.admit_refusals.store(1, SeqCst);
	let (bench, mut run) = bench(
		&lab,
		&["file_read", "file_read"],
		2,
		Some(ToolSlots::new(2)),
	);
	advance(&bench, &mut run).await.unwrap();
	assert_eq!(
		adopted(&run),
		expected(&[
			(0, json!({"error":"admission refused the call"})),
			(1, read("p1"))
		])
	);
}

#[tokio::test]
async fn a_noted_refusal_in_a_batch_pauses_after_its_siblings_are_journaled_and_resumes() {
	// Arrange: call 1 is refused by policy the first time only.
	let lab = Lab::new(|_, call| {
		async move {
			if call.path == "p1" && call.attempt == 1 {
				Err(refused_by_policy("reference.read"))
			} else {
				Ok(read(&call.path))
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
	// Assert: the named pause reaches the caller, nothing is adopted, and only the refused
	// call is REFUSED.
	let Err(Error::AuthorityRefused(refusal)) = result else {
		panic!("expected a named pause, got {result:?}");
	};
	assert_eq!(refusal.action.as_deref(), Some("reference.read"));
	assert_eq!(bench.status(&key(&run, 0)), ("COMPLETED", Some(read("p0"))));
	assert_eq!(bench.status(&key(&run, 1)).0, "REFUSED");
	assert!(bench.saved().is_empty());
	// Act: resuming opens the refused call again.
	let mut resumed = bench.admitted_run();
	advance(&bench, &mut resumed).await.unwrap();
	// Assert
	assert_eq!(lab.attempts(&key(&run, 1)), 2);
	assert_eq!(
		adopted(&resumed),
		expected(&[(0, read("p0")), (1, read("p1"))])
	);
}

#[tokio::test]
async fn a_refusal_in_compaction_is_named_after_its_stage_and_the_check_it_noted() {
	let (error, _) = refusal_note::scoped(async {
		refusal_note::set(refusal_note::Note::new(
			aidash_domain::tool::RefusalCheck::Catalog,
			Some("decision.invoke"),
		));
		at_stage("compaction")(Error::Forbidden)
	})
	.await;
	let Error::AuthorityRefused(refusal) = error else {
		panic!("expected a named pause, got {error:?}");
	};
	assert_eq!(
		refusal.to_string(),
		"compaction: Catalog Approval refused decision.invoke"
	);
	// Without a note the check is Unnamed, and every other error passes through.
	let (error, _) =
		refusal_note::scoped(async { at_stage("compaction")(Error::Unauthorized) }).await;
	assert!(matches!(
		error,
		Error::AuthorityRefused(ref refusal) if refusal.to_string() == "compaction: an authority check refused this step"
	));
	assert!(matches!(
		at_stage("compaction")(Error::Conflict("x".into())),
		Error::Conflict(_)
	));
}

fn declined(run: &mut Run, index: usize) {
	run.recovery
		.declined_calls
		.push(aidash_domain::run_state::DeclinedCall {
			key: key(run, index),
			kind: aidash_domain::tool::RefusalKind::Policy,
		});
}
fn sentence() -> String {
	aidash_domain::tool::RefusalKind::Policy.declined_sentence()
}
fn pause_of(result: Result<()>) -> aidash_domain::tool::AuthorityRefusal {
	let Err(Error::AuthorityRefused(refusal)) = result else {
		panic!("expected a pause, got {result:?}");
	};
	*refusal
}

#[tokio::test]
async fn a_gate_refusal_names_the_call_it_stopped() {
	let lab = Lab::echo();
	let (bench, mut run) = bench(&lab, &["file_read"], 1, None);
	bench.refuse("call-0");
	let refusal = pause_of(advance(&bench, &mut run).await);
	assert_eq!(
		refusal.call,
		Some(aidash_domain::tool::RefusedCall {
			key: key(&run, 0),
			index: 0,
			replay_safe: true
		})
	);
	// The gate runs before the call is journaled.
	assert!(bench.starts().is_empty());
}

#[tokio::test]
async fn a_declined_gate_refusal_reaches_the_model_without_the_gate_or_the_tool() {
	let lab = Lab::echo();
	let (bench, mut run) = bench(&lab, &["file_read"], 1, None);
	bench.refuse("call-0");
	declined(&mut run, 0);
	advance(&bench, &mut run).await.unwrap();
	assert_eq!(
		adopted(&run),
		expected(&[(0, json!({"error": sentence()}))])
	);
	// The decision is read before the gate, so the gate never refuses it again.
	assert!(bench.consulted().is_empty());
	assert!(lab.events("invoke").is_empty());
	assert!(bench.starts().is_empty());
	assert!(run.recovery.declined_calls.is_empty());
	assert_eq!(progress(&run), (1, None));
}

#[tokio::test]
async fn a_declined_refused_call_is_answered_without_running_again() {
	let lab = Lab::new(|_, _| async { Err(refused_by_policy("task.delegate")) }.boxed());
	let (bench, mut run) = bench(&lab, &["file_read"], 1, None);
	let refusal = pause_of(advance(&bench, &mut run).await);
	assert_eq!(bench.status(&key(&run, 0)).0, "REFUSED");
	assert_eq!(refusal.call.as_ref().unwrap().key, key(&run, 0));
	// The store moves the row to COMPLETED with the sentence and keeps the decision.
	bench.seed(
		&key(&run, 0),
		"COMPLETED",
		Some(json!({"error": sentence()})),
	);
	declined(&mut run, 0);
	advance(&bench, &mut run).await.unwrap();
	assert_eq!(
		adopted(&run),
		expected(&[(0, json!({"error": sentence()}))])
	);
	assert_eq!(lab.events("invoke").len(), 1);
	assert_eq!(progress(&run), (1, None));
}

#[tokio::test]
async fn the_same_refusal_after_a_decline_pauses_the_run_again() {
	let lab = Lab::echo();
	let (bench, mut run) = bench(&lab, &["file_read"], 1, None);
	bench.refuse("call-0");
	declined(&mut run, 0);
	advance(&bench, &mut run).await.unwrap();
	// The model calls again in its next response, which has its own keys.
	{
		let state = run.state.tool_mut().unwrap();
		state.cursor = 0;
		state.response_epoch = EPOCH + 1;
	}
	run.recovery.declined_calls.clear();
	let refusal = pause_of(advance(&bench, &mut run).await);
	assert_eq!(
		refusal.call.unwrap().key,
		format!("{}:{}:0", run.id, EPOCH + 1)
	);
}

#[tokio::test]
async fn a_fresh_batch_shrinks_at_a_declined_call_and_the_sequential_path_answers_it() {
	let lab = Lab::echo();
	let (bench, mut run) = bench(
		&lab,
		&["file_read", "file_read", "file_read"],
		3,
		Some(ToolSlots::new(3)),
	);
	bench.refuse("call-1");
	declined(&mut run, 1);
	advance(&bench, &mut run).await.unwrap();
	assert!(bench.admitted().is_empty());
	assert_eq!(progress(&run), (1, None));
	advance(&bench, &mut run).await.unwrap();
	assert_eq!(
		adopted(&run),
		expected(&[(0, read("p0")), (1, json!({"error": sentence()}))])
	);
	assert!(!bench.consulted().contains(&"call-1".to_owned()));
	assert!(run.recovery.declined_calls.is_empty());
}

#[tokio::test]
async fn a_resumed_batch_answers_a_declined_call_through_its_stored_result() {
	let lab = Lab::echo();
	let names = ["file_read", "file_read", "file_read"];
	let (bench, mut run) = bench(&lab, &names, 3, Some(ToolSlots::new(3)));
	run.state.tool_mut().unwrap().batch_end = Some(3);
	let stored = json!({"error": sentence()});
	bench.seed(&key(&run, 0), "STARTED", None);
	bench.seed(&key(&run, 1), "COMPLETED", Some(stored.clone()));
	bench.seed(&key(&run, 2), "STARTED", None);
	bench.refuse("call-1");
	declined(&mut run, 1);
	advance(&bench, &mut run).await.unwrap();
	assert_eq!(
		adopted(&run),
		expected(&[(0, read("p0")), (1, stored), (2, read("p2"))])
	);
	assert!(!bench.consulted().contains(&"call-1".to_owned()));
	assert!(run.recovery.declined_calls.is_empty());
	assert_eq!(progress(&run), (3, None));
}

#[tokio::test]
async fn declining_one_of_two_refused_calls_leaves_the_run_paused_on_the_other() {
	let lab = Lab::echo();
	let (bench, mut run) = bench(
		&lab,
		&["file_read", "file_read"],
		2,
		Some(ToolSlots::new(2)),
	);
	bench.refuse("call-0");
	bench.refuse("call-1");
	let first = pause_of(advance(&bench, &mut run).await);
	assert_eq!(first.call.as_ref().unwrap().index, 0);
	declined(&mut run, 0);
	advance(&bench, &mut run).await.unwrap();
	assert_eq!(
		adopted(&run),
		expected(&[(0, json!({"error": sentence()}))])
	);
	let second = pause_of(advance(&bench, &mut run).await);
	assert_eq!(second.call.as_ref().unwrap().index, 1);
	assert_eq!(second.call.unwrap().key, key(&run, 1));
}

#[tokio::test]
async fn a_newer_message_does_not_swallow_the_answer_to_a_declined_call() {
	let lab = Lab::echo();
	let (bench, mut run) = bench(&lab, &["file_read", "file_read", "file_read"], 1, None);
	bench.refuse("call-0");
	// The person declined the call at the cursor and a later one, then sent a message that the
	// stored response has not seen.
	declined(&mut run, 0);
	declined(&mut run, 2);
	bench.accept_message(
		run.state.tool().unwrap().included_input_seq + 1,
		"Try the notes.",
	);
	advance(&bench, &mut run).await.unwrap();
	// The response is discarded whole, as before, and nothing ran or consulted the gate.
	assert!(matches!(run.state, RunState::Thinking(_)));
	assert!(bench.consulted().is_empty());
	assert!(lab.events("invoke").is_empty());
	// Yet the next request holds the declined calls with their sentences, and no other call.
	assert_eq!(
		adopted(&run),
		expected(&[
			(0, json!({"error": sentence()})),
			(2, json!({"error": sentence()}))
		])
	);
	assert!(run.recovery.declined_calls.is_empty());
}

#[tokio::test]
async fn a_newer_message_leaves_an_undeclined_response_to_be_discarded_alone() {
	let lab = Lab::echo();
	let (bench, mut run) = bench(&lab, &["file_read", "file_read"], 1, None);
	bench.accept_message(
		run.state.tool().unwrap().included_input_seq + 1,
		"Try the notes.",
	);
	advance(&bench, &mut run).await.unwrap();
	assert!(matches!(run.state, RunState::Thinking(_)));
	assert!(adopted(&run).is_empty());
}
