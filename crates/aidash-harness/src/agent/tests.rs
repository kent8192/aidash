//! Agent lifecycle and inference authority are tested without a database or HTTP.
use super::*;
use aidash_application::ports::{CompactionClassifier, CompactionQuestions, ModelProvider};
use aidash_domain::context::recovery::{Attempt, Outcome, Settlement};
use aidash_domain::context::sources::{RetrievalKey, RetrievalScope};
use aidash_domain::provider::ModelContext;
use aidash_domain::provider::{ContentPart, ModelRequest, ModelResponse, ToolCall};
use aidash_domain::registry::{EntityRef, Entry};
use async_trait::async_trait;
use rstest::{fixture, rstest};
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
	/// In-memory Context Journal keyed by Run, with the same append rule as Postgres.
	journal: Mutex<Vec<(Uuid, aidash_domain::context::HistoryEntry)>>,
	attempts: Mutex<Vec<(Attempt, Option<Outcome>)>>,
	context_policy: Option<aidash_domain::context::policy::ContextPolicy>,
	/// Ordinary inference requests answered with a provider Context Overflow.
	overflows: Mutex<u32>,
	/// Jev answers every retention question with this probability; `None`
	/// makes compaction unexpected.
	jev_retention: Option<f64>,
	summary_text: Mutex<Option<String>>,
	/// The summarizer is approved at reservation but revoked during its call.
	revoke_summarizer: bool,
	/// A source an adopted Execution Summary depends on is no longer readable.
	stale_summary_dependencies: bool,
	/// Ordinary inference ends with this truncated or refused completion.
	terminal: Option<aidash_domain::context::recovery::Failure>,
	/// Summary dependency checks that pass before the sources are revoked.
	summary_checks_before_revocation: Option<usize>,
	/// The summarizer's configured `max_output_tokens`.
	summarizer_output_tokens: u32,
	projection: ProjectionVersion,
	skill_tool: bool,
	skill_revision: Mutex<Option<i64>>,
	skill_text: Mutex<String>,
	/// Returned once by the next source recheck.
	source_failure: Mutex<Option<Error>>,
	retrieval: Mutex<RetrievalScope>,
	semantic_keys: Mutex<Vec<Option<String>>>,
	inputs: Mutex<Vec<aidash_domain::run_input::RunInput>>,
	instructions: String,
	/// The semantic read returns as much content as its budget allows.
	fill_semantic_budget: bool,
	prompt_cache: aidash_domain::projection::PromptCache,
	/// Serve an `anthropic/` model that declares explicit prompt caching.
	explicit_cache_model: bool,
}
impl Backend {
	fn record(&self, name: &'static str) {
		self.0.calls.lock().unwrap().push(name);
	}
	fn task_value(&self) -> Task {
		self.0.task.lock().unwrap().clone()
	}
	fn entry(&self, id: &str) -> Entry {
		// The summarizer has a larger window so it can read absorbed history.
		let (kind, window, output) = if id == "summarizer" {
			("model", 1_000_000, self.0.summarizer_output_tokens)
		} else {
			("agent", 128_000, 4096)
		};
		let mut config = json!({
			"provider":"openrouter","model_id":"fixture","endpoint":"http://fixture.invalid/v1","credential_env":null,"context_window":window,"max_output_tokens":output,"modalities":["text"],"cost":{}
		});
		if self.0.explicit_cache_model {
			config["model_id"] = json!("anthropic/fixture");
			config["cache_mode"] = json!("explicit");
		}
		serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Fixture"},"config":config})).unwrap()
	}
	/// Journal the projection's new entries; an adoption also requires its open
	/// attempt and the attempt's source range, all-or-nothing like the store.
	fn persist(&self, run: &Run, event: &str, adopt: Option<(Uuid, &Settlement)>) -> Result<()> {
		let mut journal = self.0.journal.lock().unwrap();
		let head = journal
			.iter()
			.filter(|(id, _)| *id == run.id)
			.map(|(_, entry)| entry.seq)
			.max()
			.unwrap_or(0);
		let appended: Vec<_> = run.context.unjournaled(head).cloned().collect();
		if let Some((attempt, settlement)) = adopt {
			let mut attempts = self.0.attempts.lock().unwrap();
			let open = attempts
				.iter_mut()
				.find(|(a, outcome)| a.id == attempt && a.run_id == run.id && outcome.is_none())
				.ok_or_else(|| {
					aidash_application::Error::Conflict(
						"compaction attempt is no longer open".into(),
					)
				})?;
			if appended.last().map_or(head, |entry| entry.seq) < open.0.source_through_seq {
				return Err(aidash_application::Error::Conflict(
					"context journal lacks the compacted source range".into(),
				));
			}
			open.1 = Some(settlement.outcome);
		}
		journal.extend(appended.into_iter().map(|entry| (run.id, entry)));
		self.record("save");
		self.0
			.writes
			.lock()
			.unwrap()
			.push((event.to_owned(), run.clone()));
		Ok(())
	}
}
fn unexpected(operation: &str) -> ! {
	panic!("unexpected port call: {operation}")
}

#[async_trait]
impl ExecutionStore for Backend {
	async fn save_run(&self, run: &Run, token: Uuid, event: &str) -> Result<()> {
		assert_eq!(token, self.0.token);
		self.persist(run, event, None)
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
	async fn journal_context(&self, run: &Run, token: Uuid) -> Result<()> {
		assert_eq!(token, self.0.token);
		self.record("journal");
		let mut journal = self.0.journal.lock().unwrap();
		let head = journal
			.iter()
			.filter(|(id, _)| *id == run.id)
			.map(|(_, entry)| entry.seq)
			.max()
			.unwrap_or(0);
		let appended: Vec<_> = run.context.unjournaled(head).cloned().collect();
		journal.extend(appended.into_iter().map(|entry| (run.id, entry)));
		Ok(())
	}
	async fn context_journal(
		&self,
		run: Uuid,
		from: u64,
		through: u64,
	) -> Result<Vec<aidash_domain::context::HistoryEntry>> {
		let journal = self.0.journal.lock().unwrap();
		let mut entries: Vec<_> = journal
			.iter()
			.filter(|(id, entry)| *id == run && (from..=through).contains(&entry.seq))
			.map(|(_, entry)| entry.clone())
			.collect();
		entries.sort_by_key(|entry| entry.seq);
		Ok(entries)
	}
	async fn begin_compaction(
		&self,
		run: &Run,
		token: Uuid,
		attempt: &Attempt,
		call_budget: u32,
	) -> Result<()> {
		assert_eq!(token, self.0.token);
		assert_eq!(attempt.run_id, run.id);
		self.record("compaction.begin");
		let mut attempts = self.0.attempts.lock().unwrap();
		for (_, outcome) in attempts.iter_mut().filter(|(a, _)| a.run_id == run.id) {
			outcome.get_or_insert(Outcome::Abandoned);
		}
		let spent = attempts
			.iter()
			.filter(|(a, _)| a.run_id == run.id && a.stage == attempt.stage)
			.count();
		if spent >= call_budget as usize {
			return Err(aidash_application::Error::Context(
				aidash_domain::context::recovery::Failure::SummaryUnavailable,
			));
		}
		attempts.push((attempt.clone(), None));
		Ok(())
	}
	async fn settle_compaction(
		&self,
		run: &Run,
		token: Uuid,
		attempt: Uuid,
		settlement: &Settlement,
	) -> Result<()> {
		assert_eq!(token, self.0.token);
		assert_ne!(settlement.outcome, Outcome::Adopted);
		self.record("compaction.settle");
		if let Some((_, outcome)) = self
			.0
			.attempts
			.lock()
			.unwrap()
			.iter_mut()
			.find(|(a, outcome)| a.id == attempt && a.run_id == run.id && outcome.is_none())
		{
			*outcome = Some(settlement.outcome);
		}
		Ok(())
	}
	async fn adopt_compaction(
		&self,
		run: &Run,
		token: Uuid,
		attempt: Uuid,
		settlement: &Settlement,
	) -> Result<()> {
		assert_eq!(token, self.0.token);
		assert_eq!(settlement.outcome, Outcome::Adopted);
		self.persist(run, "context.compacted", Some((attempt, settlement)))
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
			let id: Uuid = id.parse().unwrap();
			let inputs = self.0.inputs.lock().unwrap();
			let input = inputs
				.iter()
				.find(|input| input.message_id == Some(id))
				.expect("delivered run message");
			return Ok(json!(Message {
				id,
				workspace_id: self.task_value().workspace_id,
				sender: input.sender.clone(),
				content: input.content.clone(),
				idempotency_key: Some(input.idempotency_key.clone()),
				created_at: "2026-10-02T00:00:00Z".parse().unwrap(),
			}));
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
		self.0.skill_tool
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
	async fn reserve_summary(
		&self,
		token: Uuid,
		summarizer: &aidash_domain::context::summary::SummaryProvider,
		window: usize,
		output: u32,
		request: &ModelRequest,
	) -> Result<Option<Box<dyn InferenceReservation>>> {
		let _ = (summarizer, window, output, request);
		assert_eq!(token, self.0.token);
		self.record("authority.summary_reserve");
		Ok(None)
	}
	async fn recheck_summary(
		&self,
		summarizer: &aidash_domain::context::summary::SummaryProvider,
	) -> Result<()> {
		assert_eq!(summarizer.model.id, "summarizer");
		self.record("authority.summary_recheck");
		if self.0.revoke_summarizer {
			return Err(Error::Context(
				aidash_domain::context::recovery::Failure::SummaryUnavailable,
			));
		}
		Ok(())
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
		if let Some(error) = self.0.source_failure.lock().unwrap().take() {
			return Err(error);
		}
		Ok(())
	}
	async fn skill_context(&self, run: &Run) -> Result<String> {
		let _ = run;
		if !self.0.skill_tool {
			unexpected("ExecutionEnvironment.skill_context")
		}
		self.record("source.skill");
		Ok(self.0.skill_text.lock().unwrap().clone())
	}
	async fn skill_revision(&self, run: &Run) -> Result<Option<i64>> {
		let _ = run;
		Ok(*self.0.skill_revision.lock().unwrap())
	}
	async fn cache_scope(&self, run: &Run) -> Result<aidash_domain::projection::CacheScope> {
		let _ = run;
		if self.0.projection != ProjectionVersion::Ordered {
			unexpected("ExecutionEnvironment.cache_scope")
		}
		Ok(aidash_domain::projection::CacheScope {
			tenant: "tenant-a".into(),
			key_version: 1,
		})
	}
	async fn retrieval_scope(&self, run: &Run) -> Result<RetrievalScope> {
		let _ = run;
		Ok(self.0.retrieval.lock().unwrap().clone())
	}
	async fn semantic_context(
		&self,
		run: &Run,
		task: &Task,
		inputs: &[(InputRead, String)],
		budget: usize,
		entry: &Entry,
		key: Option<&RetrievalKey>,
	) -> Result<Option<Value>> {
		let _ = (run, task, inputs, budget, entry);
		self.0
			.semantic_keys
			.lock()
			.unwrap()
			.push(key.map(RetrievalKey::digest));
		if self.0.conversation_memory {
			self.record("source.memory");
			if self.0.fill_semantic_budget {
				return Ok(Some(json!({"fact":"m".repeat(budget.saturating_sub(16))})));
			}
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
			instructions: self.0.instructions.clone(),
			knowledge_digest: None,
			tools: vec![],
			skills: vec![],
			max_steps: 64,
			allow_task_creation: None,
			conversation_memory: self.0.conversation_memory,
			context_policy: self.0.context_policy.clone(),
			projection_version: self.0.projection,
			prompt_cache: self.0.prompt_cache,
		})
	}
	fn provider(&self, _model: ModelConfig) -> Result<Arc<dyn ModelProvider>> {
		Ok(Arc::new(self.clone()))
	}
	fn compactor(&self) -> Result<Box<dyn CompactionClassifier>> {
		Ok(Box::new(self.clone()))
	}
	async fn summary_dependencies_current(
		&self,
		run: &Run,
		dependencies: &aidash_domain::context::summary::SummaryDependencies,
	) -> Result<bool> {
		let _ = (run, dependencies);
		self.record("summary.dependencies");
		let checks = self
			.0
			.calls
			.lock()
			.unwrap()
			.iter()
			.filter(|call| **call == "summary.dependencies")
			.count();
		Ok(!self.0.stale_summary_dependencies
			&& self
				.0
				.summary_checks_before_revocation
				.is_none_or(|current| checks <= current))
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
		if request.response_format.is_some() {
			request.ensure_fits(1_000_000).unwrap();
			assert!(request.tools.is_empty());
			self.record("summarizer.infer");
			self.0.requests.lock().unwrap().push(request);
			let text = self
				.0
				.summary_text
				.lock()
				.unwrap()
				.clone()
				.expect("summary fixture");
			return Ok(ModelResponse {
				text,
				input_tokens: 100,
				output_tokens: 50,
				usage_complete: true,
				..Default::default()
			});
		}
		request.ensure_fits(128000).unwrap();
		self.record("provider.infer");
		self.0.requests.lock().unwrap().push(request);
		{
			let mut overflows = self.0.overflows.lock().unwrap();
			if *overflows > 0 {
				*overflows -= 1;
				return Err(Error::ContextOverflow);
			}
		}
		if let Some(failure) = self.0.terminal {
			return Err(Error::TerminalResponse(
				failure,
				Box::new(ModelResponse {
					input_tokens: 200,
					output_tokens: 4096,
					usage_complete: true,
					..Default::default()
				}),
			));
		}
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
	async fn ask(&self, _state: &Value, questions: &CompactionQuestions) -> Result<Value> {
		let Some(retention) = self.0.jev_retention else {
			unexpected("compaction")
		};
		self.record("jev.ask");
		let answers: serde_json::Map<_, _> = questions
			.keys()
			.map(|key| (key.clone(), json!({"noul":retention})))
			.collect();
		Ok(json!({"answers":answers}))
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
		journal: Mutex::new(vec![]),
		attempts: Mutex::new(vec![]),
		context_policy: None,
		overflows: Mutex::new(0),
		jev_retention: None,
		summary_text: Mutex::new(None),
		revoke_summarizer: false,
		stale_summary_dependencies: false,
		summary_checks_before_revocation: None,
		terminal: None,
		summarizer_output_tokens: 4096,
		projection: ProjectionVersion::Legacy,
		skill_tool: false,
		skill_revision: Mutex::new(Some(1)),
		skill_text: Mutex::new("\nPinned Skills: review\n".into()),
		source_failure: Mutex::new(None),
		retrieval: Mutex::new(RetrievalScope {
			tenant: "tenant-a".into(),
			subject: "alice".into(),
			authorization_revision: Some(1),
			index_revision: Some(1),
			participant_revision: Some(1),
			corpus_digest: Some("corpus-1".into()),
		}),
		semantic_keys: Mutex::new(vec![]),
		inputs: Mutex::new(vec![]),
		instructions: "Do the task".into(),
		fill_semantic_budget: false,
		prompt_cache: Default::default(),
		explicit_cache_model: false,
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
		let mut tools = Tools::new();
		if self.0.skill_tool {
			tools.insert("skill_list".into(), Arc::new(SkillList));
		}
		Ok(tools)
	}
}
struct SkillList;
#[async_trait]
impl ExecutionTool for SkillList {
	fn specification(&self) -> aidash_domain::provider::ToolSpec {
		aidash_domain::provider::ToolSpec {
			name: "skill_list".into(),
			description: "List pinned Skills".into(),
			parameters: json!({"type":"object","properties":{}}),
		}
	}
	fn contract(&self) -> aidash_domain::tool::ToolContract {
		aidash_domain::tool::ToolContract::registry(
			EntityRef {
				id: "skill_list".into(),
				version: "1.0.0".into(),
			},
			&aidash_domain::tool::ToolConfig::Native {
				operation: "skill_list".into(),
				allowed_hosts: vec![],
			},
		)
	}
	fn replay_safe(&self) -> bool {
		true
	}
	async fn invoke(&self, _: &Run, _: Value, _: &str) -> Result<Value> {
		unexpected("skill_list.invoke")
	}
}
struct RecordReader;
#[async_trait]
impl ExecutionTool for RecordReader {
	fn specification(&self) -> aidash_domain::provider::ToolSpec {
		aidash_domain::provider::ToolSpec {
			name: "renamed_reader".into(),
			description: "Read workspace records".into(),
			parameters: json!({"type":"object","properties":{}}),
		}
	}
	fn contract(&self) -> aidash_domain::tool::ToolContract {
		aidash_domain::tool::builtin_contract("workspace_read").unwrap()
	}
	fn replay_safe(&self) -> bool {
		true
	}
	async fn invoke(&self, _: &Run, _: Value, _: &str) -> Result<Value> {
		unexpected("renamed_reader.invoke")
	}
}

#[rstest]
fn summary_dependencies_keep_a_message_whose_read_result_jev_truncated() {
	let id = Uuid::from_u128(7);
	let mut tools = Tools::new();
	tools.insert("renamed_reader".into(), Arc::new(RecordReader));
	let call = ToolCall {
		id: "read".into(),
		name: "renamed_reader".into(),
		arguments: json!({"kind":"message", "id":id, "offset":0}),
	};
	// Jev keeps the leading text of an old result as a plain string.
	let entries = [context::HistoryEntry {
		seq: 1,
		event: ContextEvent::tool(call, json!("{\"kind\":\"message\",\"content\":\"private")),
	}];

	let dependencies = summary_dependencies(&entries, &tools);

	assert!(dependencies.message_ids.contains(&id));
	assert!(dependencies.tool_call_ids.contains("read"));
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
		assert_eq!(
			serde_json::to_value(&requests[0].context).unwrap(),
			serde_json::to_value(&requests[1].context).unwrap()
		);
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
	assert_ne!(
		serde_json::to_value(&requests[1].context).unwrap(),
		serde_json::to_value(&requests[2].context).unwrap()
	);
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

fn recovery_policy(summary: bool) -> aidash_domain::context::policy::ContextPolicy {
	let mut policy = json!({"version":"context-recovery/1"});
	if summary {
		policy["summary"] = json!({"model":{"id":"summarizer","version":"1.0.0"}});
	}
	serde_json::from_value(policy).unwrap()
}

fn thinking_with_history(fixture: &mut Fixture, old: usize) {
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	fixture.run.context.push(ContextEvent::Human {
		request: "Never deploy on Fridays".into(),
		request_kind: "INFORMATION_REQUEST".into(),
		response: json!("acknowledged"),
	});
	for i in 0..old {
		fixture.run.context.push(ContextEvent::tool(
			ToolCall {
				id: format!("old-{i}"),
				name: "read".into(),
				arguments: json!({"path":format!("old-{i}")}),
			},
			json!("x".repeat(4000)),
		));
	}
	for i in 0..6 {
		fixture.run.context.push(ContextEvent::tool(
			ToolCall {
				id: format!("recent-{i}"),
				name: "read".into(),
				arguments: json!({}),
			},
			json!("recent"),
		));
	}
	// Everything above already reached an accepted inference.
	fixture.run.context.journal.inferred_through = fixture.run.context.journal.head;
}

fn events(fixture: &Fixture) -> Vec<String> {
	fixture
		.backend
		.0
		.writes
		.lock()
		.unwrap()
		.iter()
		.map(|(event, _)| event.clone())
		.collect()
}

#[rstest]
#[tokio::test]
async fn provider_overflow_retries_with_a_smaller_window_without_dispatching(mut fixture: Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.context_policy = Some(recovery_policy(false));
	*state.overflows.lock().unwrap() = 2;
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	advance_sources(&mut fixture).await.unwrap();
	// The overflow saved only recovery state; no response or tool call exists.
	assert!(matches!(fixture.run.state, RunState::Thinking(_)));
	assert_eq!(fixture.run.context.recovery.window_permille, 750);
	assert_eq!(fixture.run.context.recovery.overflow_attempts, 1);
	assert_eq!(events(&fixture).last().unwrap(), "run.context_overflow");
	// The retried request did not shrink, so recovery pauses instead of looping.
	assert!(matches!(
		advance_sources(&mut fixture).await,
		Err(Error::Context(
			aidash_domain::context::recovery::Failure::OverflowRetriesExhausted
		))
	));
	let requests = fixture.backend.0.requests.lock().unwrap();
	assert_eq!(requests.len(), 2);
	// Only the Context Policy reduces context, never OpenRouter transforms.
	assert!(
		requests
			.iter()
			.all(|request| request.disable_provider_transforms)
	);
	drop(requests);
	assert!(matches!(fixture.run.state, RunState::Thinking(_)));
}

#[rstest]
#[tokio::test]
async fn legacy_versions_and_unrelated_rejections_never_compact_and_retry(mut fixture: Fixture) {
	*fixture.backend.0.overflows.lock().unwrap() = 1;
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	assert!(matches!(
		advance_sources(&mut fixture).await,
		Err(Error::Context(
			aidash_domain::context::recovery::Failure::OverflowRetriesExhausted
		))
	));
	assert!(fixture.run.context.recovery.is_initial());
	// A prune-only Agent keeps the provider's default transforms.
	assert!(!fixture.backend.0.requests.lock().unwrap()[0].disable_provider_transforms);

	let mut rejected = fixture_with(|state| {
		state.context_policy = Some(recovery_policy(false));
		state.provider_status = Some(400);
	});
	rejected.run.state = RunState::Thinking(ThinkingState::default());
	assert!(matches!(
		advance_sources(&mut rejected).await,
		Err(Error::ProviderRejected { status: 400, .. })
	));
	assert!(rejected.run.context.recovery.is_initial());
	assert!(!events(&rejected).contains(&"run.context_overflow".to_owned()));
}

#[rstest]
#[tokio::test]
async fn summary_stage_fits_history_that_pruning_cannot_and_keeps_the_journal(
	mut fixture: Fixture,
) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	*state.summary_text.lock().unwrap() = Some(
		json!({"goal":"Fixture task","constraints":[{"id":"c1","text":"Never deploy on Fridays"}],"decisions":[],"unresolved":[{"id":"u1","text":"finish reading"}],"resolved":[],"artifacts":[],"verification":[]})
			.to_string(),
	);
	thinking_with_history(&mut fixture, 40);
	let originals = fixture.run.context.history.clone();

	// Step 1: Jev keeps everything, so the Summary Stage merges old history.
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(events(&fixture).last().unwrap(), "context.compacted");
	let summary = fixture.run.context.execution_summary.as_ref().unwrap();
	assert_eq!(
		(summary.source.from_seq, summary.source.through_seq),
		(2, 41)
	);
	assert_eq!(fixture.run.context.history.len(), 7);
	assert!(matches!(
		fixture.run.context.history[0].event,
		ContextEvent::Human { .. }
	));
	{
		let attempts = fixture.backend.0.attempts.lock().unwrap();
		assert_eq!(attempts.len(), 1);
		assert_eq!(attempts[0].1, Some(Outcome::Adopted));
	}
	// The authoritative journal still holds every original event.
	let journal = fixture
		.backend
		.context_journal(fixture.run.id, 1, u64::MAX >> 1)
		.await
		.unwrap();
	assert_eq!(journal, originals);

	// Step 2: the adopted projection fits and inference proceeds.
	advance_sources(&mut fixture).await.unwrap();
	assert!(matches!(fixture.run.state, RunState::ToolCall(_)));
	let request = fixture
		.backend
		.0
		.requests
		.lock()
		.unwrap()
		.last()
		.unwrap()
		.clone();
	assert!(request.response_format.is_none());
	let ModelContext::Legacy(context) = &request.context else {
		panic!("expected a Legacy request");
	};
	assert_eq!(context["summary"]["constraints"][0]["id"], "c1");
	assert!(!context.to_string().contains("old-0"));
}

#[rstest]
#[tokio::test]
async fn invalid_summary_leaves_the_saved_context_unchanged(mut fixture: Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	*state.summary_text.lock().unwrap() = Some("not json".into());
	thinking_with_history(&mut fixture, 40);
	let before = fixture.run.context.history.clone();
	assert!(matches!(
		advance_sources(&mut fixture).await,
		Err(Error::Context(
			aidash_domain::context::recovery::Failure::SummaryInvalid
		))
	));
	assert_eq!(fixture.run.context.history, before);
	assert!(fixture.run.context.execution_summary.is_none());
	assert!(!events(&fixture).contains(&"context.compacted".to_owned()));
	assert_eq!(
		fixture.backend.0.attempts.lock().unwrap()[0].1,
		Some(Outcome::Invalid)
	);
}

#[rstest]
#[tokio::test]
async fn summary_stage_is_unavailable_without_a_policy_model(mut fixture: Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.context_policy = Some(recovery_policy(false));
	state.jev_retention = Some(1.0);
	thinking_with_history(&mut fixture, 40);
	assert!(matches!(
		advance_sources(&mut fixture).await,
		Err(Error::Context(
			aidash_domain::context::recovery::Failure::ContextUnreducible
		))
	));
	assert!(fixture.backend.0.attempts.lock().unwrap().is_empty());
	assert!(
		!fixture
			.backend
			.0
			.calls
			.lock()
			.unwrap()
			.contains(&"summarizer.infer")
	);
}

fn valid_summary() -> String {
	json!({"goal":"Fixture task","constraints":[{"id":"c1","text":"Never deploy on Fridays"}],"decisions":[],"unresolved":[{"id":"u1","text":"finish reading"}],"resolved":[],"artifacts":[],"verification":[]})
		.to_string()
}

fn calls(fixture: &Fixture) -> Vec<&'static str> {
	fixture.backend.0.calls.lock().unwrap().clone()
}

#[rstest]
#[tokio::test]
async fn a_summarizer_revoked_during_its_call_is_never_adopted(mut fixture: Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = true;
	state.revoke_summarizer = true;
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	*state.summary_text.lock().unwrap() = Some(valid_summary());
	thinking_with_history(&mut fixture, 40);
	let before = fixture.run.context.history.clone();
	assert!(matches!(
		advance_sources(&mut fixture).await,
		Err(Error::Context(
			aidash_domain::context::recovery::Failure::SummaryUnavailable
		))
	));
	assert_eq!(fixture.run.context.history, before);
	assert!(fixture.run.context.execution_summary.is_none());
	assert!(!events(&fixture).contains(&"context.compacted".to_owned()));
	assert_eq!(
		fixture.backend.0.attempts.lock().unwrap()[0].1,
		Some(Outcome::Unauthorized)
	);
	let calls = calls(&fixture);
	let position = |name| calls.iter().position(|call| *call == name).unwrap();
	assert!(position("summarizer.infer") < position("authority.summary_recheck"));
	assert_eq!(
		calls
			.iter()
			.filter(|call| **call == "authority.summary_reserve")
			.count(),
		1,
		"the recheck charges no second call"
	);
}

#[rstest]
#[tokio::test]
async fn a_revoked_summary_dependency_pauses_before_jev_or_inference(mut fixture: Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	*state.summary_text.lock().unwrap() = Some(valid_summary());
	thinking_with_history(&mut fixture, 40);
	let originals = fixture.run.context.history.clone();
	advance_sources(&mut fixture).await.unwrap();
	assert!(fixture.run.context.execution_summary.is_some());

	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.stale_summary_dependencies = true;
	let earlier = calls(&fixture).len();
	let requests = fixture.backend.0.requests.lock().unwrap().len();
	assert!(matches!(
		advance_sources(&mut fixture).await,
		Err(Error::Forbidden)
	));
	// The restoration is saved under the step's lease; the step then pauses
	// for authority before Jev or the model can read the restored originals.
	assert_eq!(events(&fixture).last().unwrap(), "context.summary_revoked");
	assert!(fixture.run.context.execution_summary.is_none());
	assert_eq!(fixture.run.context.history, originals);
	let step = &calls(&fixture)[earlier..];
	for provider in ["jev.ask", "provider.infer", "summarizer.infer"] {
		assert!(!step.contains(&provider), "{provider}: {step:?}");
	}
	assert_eq!(fixture.backend.0.requests.lock().unwrap().len(), requests);
}

#[rstest]
#[case::truncated(aidash_domain::context::recovery::Failure::OutputTruncated)]
#[case::refused(aidash_domain::context::recovery::Failure::Refused)]
#[tokio::test]
async fn a_terminal_completion_settles_its_reservation_and_pauses(
	mut fixture: Fixture,
	#[case] failure: aidash_domain::context::recovery::Failure,
) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = true;
	state.terminal = Some(failure);
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	assert!(matches!(
		advance_sources(&mut fixture).await,
		Err(Error::Context(actual)) if actual == failure
	));
	let calls = calls(&fixture);
	let position = |name| calls.iter().position(|call| *call == name).unwrap();
	// The reported usage settles the reservation instead of keeping the
	// whole window charged; no tool call or response is accepted.
	assert!(position("provider.infer") < position("reservation.settle"));
	assert!(matches!(fixture.run.state, RunState::Thinking(_)));
}

#[rstest]
#[tokio::test]
async fn a_summary_dependency_revoked_during_pruning_pauses_before_inference(mut fixture: Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = true;
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	*state.summary_text.lock().unwrap() = Some(valid_summary());
	thinking_with_history(&mut fixture, 40);
	let originals = fixture.run.context.history.clone();
	advance_sources(&mut fixture).await.unwrap();
	assert!(fixture.run.context.execution_summary.is_some());

	// The check before pruning passes; the source is revoked before the
	// check that follows it, while authority was released for Jev.
	let earlier = calls(&fixture).len();
	let checks = calls(&fixture)
		.iter()
		.filter(|call| **call == "summary.dependencies")
		.count();
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.summary_checks_before_revocation = Some(checks + 1);
	let requests = fixture.backend.0.requests.lock().unwrap().len();
	assert!(matches!(
		advance_sources(&mut fixture).await,
		Err(Error::Forbidden)
	));
	assert_eq!(events(&fixture).last().unwrap(), "context.summary_revoked");
	assert!(fixture.run.context.execution_summary.is_none());
	assert_eq!(fixture.run.context.history, originals);
	let step = &calls(&fixture)[earlier..];
	assert_eq!(
		step.iter()
			.filter(|call| **call == "summary.dependencies")
			.count(),
		2,
		"{step:?}"
	);
	for provider in ["provider.infer", "summarizer.infer"] {
		assert!(!step.contains(&provider), "{provider}: {step:?}");
	}
	assert_eq!(fixture.backend.0.requests.lock().unwrap().len(), requests);
}

#[rstest]
#[tokio::test]
async fn a_source_revoked_while_the_summarizer_runs_is_never_adopted(mut fixture: Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = true;
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	*state.summary_text.lock().unwrap() = Some(valid_summary());
	// No summary exists yet, so the only check is the one after the
	// summarizer call, made once authority resumes; the source is revoked.
	state.summary_checks_before_revocation = Some(0);
	thinking_with_history(&mut fixture, 40);
	let before = fixture.run.context.history.clone();

	assert!(matches!(
		advance_sources(&mut fixture).await,
		Err(Error::Forbidden)
	));

	assert!(fixture.run.context.execution_summary.is_none());
	assert_eq!(fixture.run.context.history, before);
	assert!(!events(&fixture).contains(&"context.compacted".to_owned()));
	{
		let attempts = fixture.backend.0.attempts.lock().unwrap();
		assert_eq!(attempts.len(), 1);
		assert_eq!(attempts[0].1, Some(Outcome::Unauthorized));
	}
	let calls = calls(&fixture);
	let position = |name| calls.iter().position(|call| *call == name).unwrap();
	assert!(position("summarizer.infer") < position("summary.dependencies"));
	assert!(!calls.contains(&"provider.infer"));
}

#[rstest]
#[tokio::test]
async fn imported_history_is_journaled_whole_before_pruning(mut fixture: Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.context_policy = Some(recovery_policy(false));
	state.jev_retention = Some(0.0);
	thinking_with_history(&mut fixture, 40);
	// A pre-journal Run stores bare events and no cursor; loading imports them.
	let mut stored = serde_json::to_value(&fixture.run.context).unwrap();
	stored["history"] = json!(fixture.run.context.events().collect::<Vec<_>>());
	stored.as_object_mut().unwrap().remove("journal");
	fixture.run.context = serde_json::from_value(stored).unwrap();
	let originals = fixture.run.context.history.clone();
	assert_eq!(
		fixture.run.context.journal.imported_through,
		originals.len() as u64
	);
	// A cached source observation means no save precedes compaction.
	fixture.run.context.source_observation = Some(
		aidash_domain::context::sources::SourceObservation::new(
			"0:0:0".into(),
			aidash_domain::registry::rules::digest(&json!("null")),
			json!({"memory":null,"skill_context":"","semantic_memory":null}),
		)
		.unwrap(),
	);
	advance_sources(&mut fixture).await.unwrap();
	assert!(!events(&fixture).contains(&"run.sources_observed".to_owned()));
	assert!(
		fixture.run.context.history.len() < originals.len(),
		"Jev pruned the saved projection"
	);
	let journal = fixture
		.backend
		.context_journal(fixture.run.id, 1, u64::MAX >> 1)
		.await
		.unwrap();
	assert_eq!(journal, originals);
	let calls = calls(&fixture);
	let position = |name| calls.iter().position(|call| *call == name).unwrap();
	assert!(position("journal") < position("jev.ask"));
}

#[rstest]
#[tokio::test]
async fn summary_output_is_capped_at_the_summarizer_limit(mut fixture: Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	state.summarizer_output_tokens = 1024;
	*state.summary_text.lock().unwrap() = Some(valid_summary());
	thinking_with_history(&mut fixture, 40);
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(events(&fixture).last().unwrap(), "context.compacted");
	let requests = fixture.backend.0.requests.lock().unwrap();
	let summary = requests
		.iter()
		.find(|request| request.response_format.is_some())
		.unwrap();
	// The policy default (4096) is clamped to the pinned summarizer's limit.
	assert_eq!(summary.max_output_tokens, 1024);
}

fn fixture_with(configure: impl FnOnce(&mut State)) -> Fixture {
	let mut fixture = fixture();
	configure(Arc::get_mut(&mut fixture.backend.0).unwrap());
	fixture
}

/// Fixed identities so request bytes can be compared with checked-in fixtures.
#[fixture]
fn canonical(mut fixture: Fixture) -> Fixture {
	fixture.run.id = Uuid::from_u128(0x172);
	fixture.run.workspace_id = Uuid::from_u128(0x2);
	fixture.run.task_id = Uuid::from_u128(0x3);
	{
		let mut task = fixture.backend.0.task.lock().unwrap();
		task.id = fixture.run.task_id;
		task.workspace_id = fixture.run.workspace_id;
	}
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.conversation_memory = true;
	// The request is recorded before the provider fails, so each advance is
	// one inference boundary that leaves the Run in Thinking.
	state.provider_status = Some(503);
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	fixture.run.context.push(history_event(1));
	fixture
}

fn history_event(n: u32) -> ContextEvent {
	ContextEvent::tool(
		ToolCall {
			id: format!("call-{n}"),
			name: "workspace_read".into(),
			arguments: json!({"kind":"task"}),
		},
		json!({"title":"Task","read":n}),
	)
}

fn ordered(fixture: &mut Fixture) {
	Arc::get_mut(&mut fixture.backend.0).unwrap().projection = ProjectionVersion::Ordered;
}

fn ordered_parts(request: &ModelRequest) -> (&str, &str) {
	match &request.context {
		ModelContext::Ordered(context) => (&context.stable, &context.volatile),
		ModelContext::Legacy(_) => panic!("expected an Ordered request"),
	}
}

fn semantic_memory(request: &ModelRequest) -> Value {
	serde_json::from_str::<Value>(ordered_parts(request).1).unwrap()["semantic_memory"].clone()
}

fn count(fixture: &Fixture, call: &str) -> usize {
	fixture
		.backend
		.0
		.calls
		.lock()
		.unwrap()
		.iter()
		.filter(|name| **name == call)
		.count()
}

#[rstest]
#[tokio::test]
async fn legacy_request_bytes_and_digest_match_the_base_projection(mut canonical: Fixture) {
	assert!(advance_sources(&mut canonical).await.is_err());
	let requests = canonical.backend.0.requests.lock().unwrap();
	assert_eq!(
		requests[0].input_body().to_string(),
		include_str!("fixtures/legacy_request.json").trim_end()
	);
	assert_eq!(
		requests[0].inference_digest(),
		include_str!("fixtures/legacy_inference_digest.txt").trim_end()
	);
}

#[rstest]
#[tokio::test]
async fn consecutive_ordered_steps_extend_the_stable_prefix(mut canonical: Fixture) {
	ordered(&mut canonical);
	Arc::get_mut(&mut canonical.backend.0).unwrap().skill_tool = true;
	assert!(advance_sources(&mut canonical).await.is_err());
	canonical.run.step += 1;
	canonical.run.context.push(history_event(2));
	assert!(advance_sources(&mut canonical).await.is_err());

	let requests = canonical.backend.0.requests.lock().unwrap();
	let (first, second) = (&requests[0], &requests[1]);
	assert_eq!(
		first.input_body().to_string(),
		include_str!("fixtures/ordered_request.json").trim_end()
	);
	assert_eq!(first.instructions, second.instructions);
	assert_eq!(
		serde_json::to_string(&first.tools).unwrap(),
		serde_json::to_string(&second.tools).unwrap()
	);
	let (stable, volatile) = ordered_parts(first);
	let (next_stable, next_volatile) = ordered_parts(second);
	assert!(next_stable.starts_with(stable.strip_suffix("]}").unwrap()));
	assert_ne!(stable, next_stable);
	assert_ne!(volatile, next_volatile);
	assert_eq!(
		first.cache_scope,
		Some(aidash_domain::projection::CacheScope {
			tenant: "tenant-a".into(),
			key_version: 1
		})
	);
	drop(requests);
	assert_eq!(count(&canonical, "source.skill"), 1);
	assert_eq!(count(&canonical, "source.memory"), 1);
}

#[rstest]
#[case::same_key(|_: &mut Fixture| {}, false)]
#[case::query_input(|f: &mut Fixture| f.backend.0.task.lock().unwrap().description = "Changed".into(), true)]
#[case::authorization_revision(|f: &mut Fixture| f.backend.0.retrieval.lock().unwrap().authorization_revision = Some(2), true)]
#[case::index_revision(|f: &mut Fixture| f.backend.0.retrieval.lock().unwrap().index_revision = Some(2), true)]
#[case::participant_revision(|f: &mut Fixture| f.backend.0.retrieval.lock().unwrap().participant_revision = None, true)]
#[case::corpus(|f: &mut Fixture| f.backend.0.retrieval.lock().unwrap().corpus_digest = Some("corpus-2".into()), true)]
#[case::tenant(|f: &mut Fixture| f.backend.0.retrieval.lock().unwrap().tenant = "tenant-b".into(), true)]
#[case::subject(|f: &mut Fixture| f.backend.0.retrieval.lock().unwrap().subject = "bob".into(), true)]
#[tokio::test]
async fn ordered_semantic_read_is_reused_only_under_the_same_retrieval_key(
	mut canonical: Fixture,
	#[case] change: fn(&mut Fixture),
	#[case] retrieves: bool,
) {
	ordered(&mut canonical);
	assert!(advance_sources(&mut canonical).await.is_err());
	*canonical.backend.0.memory_value.lock().unwrap() = json!({"fact":"changed"});
	change(&mut canonical);
	canonical.run.step += 1;
	assert!(advance_sources(&mut canonical).await.is_err());

	assert_eq!(
		count(&canonical, "source.memory"),
		1 + usize::from(retrieves)
	);
	let requests = canonical.backend.0.requests.lock().unwrap();
	assert_eq!(semantic_memory(&requests[0]), json!({"fact":"observed"}));
	assert_eq!(
		semantic_memory(&requests[1]),
		if retrieves {
			json!({"fact":"changed"})
		} else {
			json!({"fact":"observed"})
		}
	);
	let keys = canonical.backend.0.semantic_keys.lock().unwrap();
	assert!(keys.iter().all(Option::is_some));
	if retrieves {
		assert_ne!(keys[0], keys[1]);
	}
}

#[rstest]
#[tokio::test]
async fn forbidden_recheck_stops_ordered_reuse_before_inference(mut canonical: Fixture) {
	ordered(&mut canonical);
	assert!(advance_sources(&mut canonical).await.is_err());
	Arc::get_mut(&mut canonical.backend.0).unwrap().deny_source = true;
	canonical.run.step += 1;
	assert!(matches!(
		advance_sources(&mut canonical).await,
		Err(Error::Forbidden)
	));
	assert_eq!(canonical.backend.0.requests.lock().unwrap().len(), 1);
	assert_eq!(count(&canonical, "source.memory"), 1);
}

#[rstest]
#[tokio::test]
async fn conflicting_recheck_discards_the_ordered_cache_and_retrieves_once(mut canonical: Fixture) {
	ordered(&mut canonical);
	assert!(advance_sources(&mut canonical).await.is_err());
	*canonical.backend.0.memory_value.lock().unwrap() = json!({"fact":"changed"});
	*canonical.backend.0.source_failure.lock().unwrap() =
		Some(Error::Conflict("observed semantic index changed".into()));
	canonical.run.step += 1;
	assert!(matches!(
		advance_sources(&mut canonical).await,
		Err(Error::ProviderRejected { .. })
	));

	assert_eq!(count(&canonical, "source.memory"), 2);
	assert_eq!(
		semantic_memory(&canonical.backend.0.requests.lock().unwrap()[1]),
		json!({"fact":"changed"})
	);
	let writes = canonical.backend.0.writes.lock().unwrap();
	let observed = writes
		.iter()
		.filter(|(event, _)| event == "run.sources_observed")
		.collect::<Vec<_>>();
	assert_eq!(observed.len(), 2);
	assert_eq!(
		observed[1]
			.1
			.context
			.source_observation
			.as_ref()
			.unwrap()
			.content["semantic_memory"],
		json!({"fact":"changed"})
	);
}

#[rstest]
#[tokio::test]
async fn conflicting_recheck_still_stops_a_legacy_run(mut canonical: Fixture) {
	assert!(advance_sources(&mut canonical).await.is_err());
	*canonical.backend.0.source_failure.lock().unwrap() =
		Some(Error::Conflict("observed semantic index changed".into()));
	assert!(matches!(
		advance_sources(&mut canonical).await,
		Err(Error::Conflict(_))
	));
	assert_eq!(canonical.backend.0.requests.lock().unwrap().len(), 1);
}

#[rstest]
#[tokio::test]
async fn ordered_observation_is_never_reused_by_another_run(mut canonical: Fixture) {
	ordered(&mut canonical);
	assert!(advance_sources(&mut canonical).await.is_err());
	let observed = canonical.run.context.clone();
	assert!(observed.source_observation.is_some());
	canonical.run.id = Uuid::from_u128(0x173);
	canonical.run.context = observed;
	assert!(advance_sources(&mut canonical).await.is_err());
	assert_eq!(count(&canonical, "source.memory"), 2);
}

#[rstest]
#[tokio::test]
async fn recovered_ordered_run_resumes_with_identical_request_bytes(mut canonical: Fixture) {
	ordered(&mut canonical);
	Arc::get_mut(&mut canonical.backend.0).unwrap().skill_tool = true;
	assert!(advance_sources(&mut canonical).await.is_err());
	let saved = canonical
		.backend
		.0
		.writes
		.lock()
		.unwrap()
		.last()
		.unwrap()
		.1
		.clone();
	canonical.run = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
	*canonical.backend.0.memory_value.lock().unwrap() = json!({"fact":"changed"});
	*canonical.backend.0.skill_text.lock().unwrap() = "\nPinned Skills: changed\n".into();
	assert!(advance_sources(&mut canonical).await.is_err());

	let requests = canonical.backend.0.requests.lock().unwrap();
	assert_eq!(
		requests[0].input_body().to_string(),
		requests[1].input_body().to_string()
	);
	drop(requests);
	assert_eq!(count(&canonical, "source.memory"), 1);
	assert_eq!(count(&canonical, "source.skill"), 1);
}

#[rstest]
#[tokio::test]
async fn changed_skill_record_revision_reloads_only_the_skill_context(mut canonical: Fixture) {
	ordered(&mut canonical);
	Arc::get_mut(&mut canonical.backend.0).unwrap().skill_tool = true;
	assert!(advance_sources(&mut canonical).await.is_err());
	*canonical.backend.0.skill_revision.lock().unwrap() = Some(2);
	*canonical.backend.0.skill_text.lock().unwrap() = "\nPinned Skills: changed\n".into();
	canonical.run.step += 1;
	assert!(advance_sources(&mut canonical).await.is_err());

	assert_eq!(count(&canonical, "source.skill"), 2);
	assert_eq!(count(&canonical, "source.memory"), 1);
	let requests = canonical.backend.0.requests.lock().unwrap();
	assert!(requests[1].instructions.contains("Pinned Skills: changed"));
}

#[rstest]
#[case::single_event(1)]
#[case::long_history(64)]
fn ordered_estimate_counts_the_complete_request_near_the_window(#[case] events: u32) {
	let scope = context::RequestProjection::Ordered(aidash_domain::projection::CacheScope {
		tenant: "tenant-a".into(),
		key_version: 1,
	});
	let legacy = context::RequestProjection::Legacy;
	let pinned = json!({
		"identity":{"node_id":"aidash://fixture","agent_id":"agent","agent_version":"1.0.0"},
		"task":{"title":"Task","description":"x".repeat(4096)},
		"workspace":{"workspace_id":Uuid::nil()},
		"agent_state":{"phase":"thinking","step":3},
		"semantic_memory":{"fact":"observed"},
	});
	let mut context = Context::default();
	for n in 1..=events {
		context.push(history_event(n));
	}
	let request = |projection| {
		context::RequestBudget {
			window: 128_000,
			instructions: "Do the task",
			tools: &[],
			max_output_tokens: 4096,
			projection,
		}
		.request(&context, &pinned)
	};
	let ordered = request(&scope);
	let estimate = ordered.estimated_total_tokens();
	// Ordered estimates count cache breakpoint framing whether or not a step
	// sends it, so the estimate equals the marked request (ADR 0019).
	let mut marked = ordered.clone();
	marked.cache_breakpoints = true;
	assert_eq!(marked.estimated_total_tokens(), estimate);
	assert_eq!(
		estimate,
		marked.input_body().to_string().len()
			+ aidash_domain::projection::CACHE_SALT_LINE_RESERVE
			+ 4096 + 1024
	);
	// The final fitting decision uses the same complete estimate.
	assert!(ordered.ensure_fits(estimate).is_ok());
	assert!(ordered.ensure_fits(estimate - 1).is_err());
	// History grows both projections by the same bytes, so Ordered reaches the
	// compaction threshold at the same history size as Legacy, offset only by
	// fixed framing and the salt reserve.
	let mut grown = context.clone();
	grown.push(history_event(events + 1));
	let growth = |projection| {
		let budget = context::RequestBudget {
			window: 128_000,
			instructions: "Do the task",
			tools: &[],
			max_output_tokens: 4096,
			projection,
		};
		budget.request(&grown, &pinned).estimated_total_tokens()
			- budget.request(&context, &pinned).estimated_total_tokens()
	};
	assert_eq!(growth(&scope), growth(&legacy));
	assert_eq!(
		growth(&scope),
		context::tool_event_growth(&context, &history_event(events + 1))
	);
}

/// Fixed content that leaves less headroom than the Run-stable quota must
/// truncate the task snapshot, not admit a request that cannot fit.
#[rstest]
#[case::legacy(false)]
#[case::ordered(true)]
#[tokio::test]
async fn oversized_fixed_content_truncates_the_snapshot_to_fit(
	mut canonical: Fixture,
	#[case] is_ordered: bool,
) {
	if is_ordered {
		ordered(&mut canonical);
	}
	Arc::get_mut(&mut canonical.backend.0).unwrap().instructions = "x".repeat(100_000);
	// Smaller than the Run-stable quota, larger than the remaining headroom.
	let quota = context::ordered_stable_quota(128_000, 4096);
	canonical.backend.0.task.lock().unwrap().description = "d".repeat(quota - 4096);
	assert!(matches!(
		advance_sources(&mut canonical).await,
		Err(Error::ProviderRejected { .. })
	));
	let requests = canonical.backend.0.requests.lock().unwrap();
	let content = requests[0].input_body().to_string();
	assert!(content.contains("snapshot_truncated"));
	assert!(requests[0].ensure_fits(128_000).is_ok());
}

/// A semantic read that fills its whole budget must still fit beside fixed
/// content that leaves less headroom than the Run-fixed semantic budget.
#[rstest]
#[case::legacy(false)]
#[case::ordered(true)]
#[tokio::test]
async fn semantic_budget_never_exceeds_the_remaining_headroom(
	mut canonical: Fixture,
	#[case] is_ordered: bool,
) {
	if is_ordered {
		ordered(&mut canonical);
	}
	let state = Arc::get_mut(&mut canonical.backend.0).unwrap();
	state.fill_semantic_budget = true;
	// Leaves less request headroom than ordered_semantic_budget(128000, 4096).
	state.instructions = "x".repeat(110_000);
	assert!(matches!(
		advance_sources(&mut canonical).await,
		Err(Error::ProviderRejected { .. })
	));
	let requests = canonical.backend.0.requests.lock().unwrap();
	assert!(requests[0].ensure_fits(128_000).is_ok());
	assert!(requests[0].input_body().to_string().contains("mmmm"));
}

/// One plain step, then a run-message catch-up step on the same Run.
async fn catch_up_requests(mut fixture: Fixture) -> (ModelRequest, ModelRequest) {
	assert!(advance_sources(&mut fixture).await.is_err());
	*fixture.backend.0.inputs.lock().unwrap() = vec![aidash_domain::run_input::RunInput {
		seq: 2,
		sender: "human".into(),
		content: "Also cover the edge cases".into(),
		idempotency_key: "input-2".into(),
		message_id: Some(Uuid::from_u128(0x22)),
		reference_only: false,
	}];
	// An earlier summarized page puts the next accepted input into catch-up.
	fixture.run.context.run_message_summary_seq = 1;
	fixture.run.step += 1;
	assert!(advance_sources(&mut fixture).await.is_err());
	let requests = fixture.backend.0.requests.lock().unwrap();
	(requests[0].clone(), requests[1].clone())
}

#[tokio::test]
async fn ordered_catch_up_keeps_its_turn_paragraph_out_of_the_system_prompt() {
	let (legacy_plain, legacy_catch_up) = catch_up_requests(canonical(fixture())).await;
	let catch_up_paragraph = legacy_catch_up
		.instructions
		.strip_prefix(&legacy_plain.instructions)
		.unwrap();
	assert!(!catch_up_paragraph.trim().is_empty());

	let mut ordered_fixture = canonical(fixture());
	ordered(&mut ordered_fixture);
	let (plain, catch_up) = catch_up_requests(ordered_fixture).await;
	assert_eq!(plain.instructions, catch_up.instructions);
	assert_eq!(plain.instructions, legacy_plain.instructions);
	let volatile =
		|request: &ModelRequest| serde_json::from_str::<Value>(ordered_parts(request).1).unwrap();
	assert!(volatile(&plain).get("turn_instructions").is_none());
	assert_eq!(
		volatile(&catch_up)["turn_instructions"],
		json!(catch_up_paragraph.trim_start())
	);
}

/// An Agent opted in to explicit prompt caching on an `anthropic/` model that
/// declares `explicit`, on the given Projection Version.
fn explicit_cache(fixture: &mut Fixture, projection: ProjectionVersion) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.projection = projection;
	state.prompt_cache = aidash_domain::projection::PromptCache::Explicit;
	state.explicit_cache_model = true;
}

#[rstest]
#[case::opted_in(aidash_domain::projection::PromptCache::Explicit, true)]
#[case::not_opted_in(aidash_domain::projection::PromptCache::Off, false)]
#[tokio::test]
async fn ordered_requests_carry_breakpoints_only_for_an_opted_in_agent(
	mut canonical: Fixture,
	#[case] prompt_cache: aidash_domain::projection::PromptCache,
	#[case] expected: bool,
) {
	// Arrange
	explicit_cache(&mut canonical, ProjectionVersion::Ordered);
	Arc::get_mut(&mut canonical.backend.0).unwrap().prompt_cache = prompt_cache;

	// Act
	assert!(advance_sources(&mut canonical).await.is_err());

	// Assert
	let requests = canonical.backend.0.requests.lock().unwrap();
	assert_eq!(requests.len(), 1);
	assert_eq!(requests[0].cache_breakpoints, expected);
	let body = requests[0].input_body();
	assert_eq!(
		body.to_string().matches("cache_control").count(),
		if expected { 2 } else { 0 }
	);
	assert_eq!(
		body["messages"][1]["content"][0]
			.get("cache_control")
			.is_some(),
		expected,
		"the Stable Prefix part"
	);
	assert!(
		body["messages"][1]["content"][1]
			.get("cache_control")
			.is_none(),
		"the volatile part"
	);
}

#[rstest]
#[case::legacy_agent(ProjectionVersion::Legacy, true)]
#[case::undeclared_model(ProjectionVersion::Ordered, false)]
#[tokio::test]
async fn an_explicit_opt_in_never_reaches_an_unsupported_request(
	mut canonical: Fixture,
	#[case] projection: ProjectionVersion,
	#[case] explicit_cache_model: bool,
) {
	// Arrange
	explicit_cache(&mut canonical, projection);
	Arc::get_mut(&mut canonical.backend.0)
		.unwrap()
		.explicit_cache_model = explicit_cache_model;

	// Act
	let error = advance_sources(&mut canonical).await.unwrap_err();

	// Assert
	assert!(
		matches!(&error, Error::Domain(aidash_domain::Error::Invalid(message)) if message.starts_with("Agent prompt_cache explicit requires")),
		"{error:?}"
	);
	assert!(canonical.backend.0.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn catch_up_steps_carry_no_breakpoints() {
	// Arrange
	let mut fixture = canonical(fixture());
	explicit_cache(&mut fixture, ProjectionVersion::Ordered);

	// Act
	let (plain, catch_up) = catch_up_requests(fixture).await;

	// Assert
	assert!(plain.cache_breakpoints);
	assert!(!catch_up.cache_breakpoints);
	assert!(!catch_up.input_body().to_string().contains("cache_control"));
	assert_eq!(
		catch_up.estimated_total_tokens(),
		ModelRequest {
			cache_breakpoints: true,
			..catch_up.clone()
		}
		.estimated_total_tokens()
	);
}
