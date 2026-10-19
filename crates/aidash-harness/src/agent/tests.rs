//! Agent lifecycle and inference authority are tested without a database or HTTP.
use super::*;
use aidash_application::ports::{InferenceProgressSink, ModelProvider};
use aidash_domain::capabilities::skills::SkillMetadata;
use aidash_domain::context::recovery::{Attempt, Outcome, Settlement};
use aidash_domain::context::sources::{RetrievalKey, RetrievalScope};
use aidash_domain::exposure::{
	CapabilityKind, DirectSkill, ExposureState, ExposureUpdate, Loaded, SkillOriginKind,
};
use aidash_domain::provider::ModelContext;
use aidash_domain::provider::progress::{
	FLUSH_INTERVAL, InferenceProgress, MAX_PENDING_BYTES, MAX_PROGRESS_ITEM_BYTES,
};
use aidash_domain::provider::{
	ContentPart, ModelRequest, ModelResponse, ToolCall, ToolSpec,
	usage::{EstimateConfidence, ReportedUsage, UsageDispatch, UsageOutcome},
};
use aidash_domain::registry::bindings::{
	BindingOrigin, DEFAULT_TOOLS, EXPOSURE_TOOLS, Narrowing, QualifiedRef, REQUIRED_TOOLS,
	ResolvedBinding, ResolvedDefinition, SKILL_ASSET_READ, SKILL_TOOLS,
};
use aidash_domain::registry::{EntityRef, Entry, rules};
use aidash_domain::run_input::RunInput;
use aidash_domain::tool::providers::core_descriptor;
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One scripted action of the fake provider during a single inference.
enum Step {
	Offer(InferenceProgress),
	Sleep(Duration),
	/// Accept a new Run input, as a human correction would.
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
	/// In-memory Context Journal keyed by Run, with the same append rule as Postgres.
	journal: Mutex<Vec<(Uuid, aidash_domain::context::HistoryEntry)>>,
	compaction_attempts: Mutex<Vec<(Attempt, Option<Outcome>)>>,
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
	/// When set, revocation past that threshold hits only the dependency sets
	/// that contain this message.
	revoked_message: Option<Uuid>,
	/// The summary reservation holds a charge that a blocked call must release.
	summary_reservation: bool,
	/// The summarizer's configured `max_output_tokens`.
	summarizer_output_tokens: u32,
	/// Provider scripts consumed one per inference; an empty queue answers at once.
	scripts: Mutex<VecDeque<Vec<Step>>>,
	attempts: Mutex<Vec<(InferenceAttemptId, Vec<ProgressOutcome>)>>,
	progress: Mutex<Vec<(InferenceAttemptId, Vec<InferenceProgress>)>>,
	interruption: tokio::sync::watch::Sender<Option<InferenceInterruption>>,
	deny_recheck: bool,
	fail_append: bool,
	/// Appends never complete, as when another transaction holds the Run row.
	block_append: bool,
	cancel_inference: bool,
	usage: Mutex<Vec<(UsageDispatch, Option<UsageOutcome>)>>,
	stall_provider: AtomicBool,
	omit_output_usage: bool,
	tools: Vec<&'static str>,
	skills: Vec<EntityRef>,
	skill_context: Option<String>,
	local_authority: bool,
	messages: Vec<Message>,
	/// Tools that replace or extend the builtin fixtures, by alias.
	custom: BTreeMap<String, Arc<Scripted>>,
	/// Journaled invocations by key: status and result.
	invocations: Mutex<BTreeMap<String, (String, Option<Value>)>>,
	direct_skills: Vec<DirectSkill>,
	direct_bodies: BTreeMap<Uuid, String>,
	projection: ProjectionVersion,
	skill_tool: bool,
	skill_revision: Mutex<Option<i64>>,
	skill_text: Mutex<String>,
	/// Returned once by the next source recheck.
	source_failure: Mutex<Option<Error>>,
	retrieval: Mutex<RetrievalScope>,
	semantic_keys: Mutex<Vec<Option<String>>>,
	/// `degrade_on_outage` of each semantic retrieval, in call order.
	semantic_degrade: Mutex<Vec<bool>>,
	inputs: Mutex<Vec<aidash_domain::run_input::RunInput>>,
	instructions: String,
	/// The semantic read returns as much content as its budget allows.
	fill_semantic_budget: bool,
	/// Run messages carry media whose intake continues past this request.
	human_media_pending: bool,
	prompt_cache: aidash_domain::projection::PromptCache,
	/// Serve an `anthropic/` model that declares explicit prompt caching.
	explicit_cache_model: bool,
	/// Evidence fragments the Run's evidence store holds.
	known_evidence: Vec<aidash_domain::research::citations::EvidenceRef>,
	/// The Run's Research evidence was revoked.
	research_revoked: bool,
	/// Revoked only once this many revocation checks have answered.
	research_revoked_after: Option<usize>,
	research_checks: std::sync::atomic::AtomicUsize,
	/// Provider results the Research account terms no longer keep.
	expired_research: aidash_domain::research::expiry::ExpiredResults,
	/// Completed artifacts, in completion order.
	artifacts: Mutex<Vec<Value>>,
	completion: Option<aidash_domain::completion::CompletionPolicy>,
	/// Completion Gate Checks; `None` means the node cannot run them.
	checks: Option<completion::FakeChecks>,
	/// The Completion Basis each commit fence persisted.
	fenced: Mutex<Vec<Option<aidash_domain::completion::CompletionBasis>>>,
	/// The `request_estimator` the served model version declares.
	request_estimator: Option<Value>,
	/// Input tokens the provider reports; 200 when unset.
	reported_input_tokens: Option<u64>,
	/// Calibration samples the store returns, newest first.
	calibration: Mutex<Vec<aidash_domain::provider::estimate::CalibrationSample>>,
	/// URL intents recorded beside the Run's Task and inputs.
	url_intents: Vec<aidash_domain::research::intent::UrlIntent>,
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
			"provider":"openrouter","model_id":"fixture","endpoint":"http://fixture.invalid/v1","context_window":window,"max_output_tokens":output,"modalities":["text"],"cost":{}
		});
		if self.0.explicit_cache_model {
			config["model_id"] = json!("anthropic/fixture");
			config["cache_mode"] = json!("explicit");
		}
		if let Some(estimator) = self
			.0
			.request_estimator
			.as_ref()
			.filter(|_| id != "summarizer")
		{
			config["request_estimator"] = estimator.clone();
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
			let mut attempts = self.0.compaction_attempts.lock().unwrap();
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
		if event == "model.completed" {
			// The store closes the pending attempt as accepted in the same transaction.
			let mut attempts = self.0.attempts.lock().unwrap();
			if let Some((_, outcomes)) = attempts.iter_mut().rfind(|(_, o)| o.is_empty()) {
				outcomes.push(ProgressOutcome::Accepted);
			}
		}
		self.persist(run, event, None)
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
		if self.0.block_append {
			return std::future::pending().await;
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
	async fn research_revoked(&self, _: &Run) -> Result<bool> {
		self.record("store.research_revoked");
		let checks = self
			.0
			.research_checks
			.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
		Ok(self.0.research_revoked
			|| self
				.0
				.research_revoked_after
				.is_some_and(|after| checks >= after))
	}
	async fn expired_research(
		&self,
		_: &Run,
	) -> Result<aidash_domain::research::expiry::ExpiredResults> {
		self.record("store.expired_research");
		Ok(self.0.expired_research.clone())
	}
	async fn unknown_evidence(
		&self,
		_: &Run,
		references: &[aidash_domain::research::citations::EvidenceRef],
	) -> Result<Vec<aidash_domain::research::citations::EvidenceRef>> {
		self.record("store.evidence");
		Ok(references
			.iter()
			.filter(|reference| !self.0.known_evidence.contains(reference))
			.copied()
			.collect())
	}
	async fn run_inputs(&self, run: Uuid) -> Result<Vec<aidash_domain::run_input::RunInput>> {
		let _ = run;
		Ok(self.0.inputs.lock().unwrap().clone())
	}
	async fn begin_final_completion(&self, run: &Run, token: Uuid) -> Result<bool> {
		assert_eq!(token, self.0.token);
		self.record("completion.fence");
		if self.0.permit_completion {
			self.0
				.fenced
				.lock()
				.unwrap()
				.push(run.context.completion.basis);
		}
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
	async fn invocation_start_batch(
		&self,
		run: &Run,
		token: Uuid,
		calls: &[BatchInvocation<'_>],
	) -> Result<Vec<InvocationOutcome>> {
		let _ = (run, token, calls);
		unexpected("ExecutionStore.invocation_start_batch")
	}
	async fn invocation_finish(
		&self,
		run: &Run,
		token: Uuid,
		key: &str,
		output: &Value,
	) -> Result<Value> {
		let _ = run;
		assert_eq!(token, self.0.token);
		self.record("invocation.finish");
		self.0
			.invocations
			.lock()
			.unwrap()
			.insert(key.to_owned(), ("COMPLETED".into(), Some(output.clone())));
		Ok(output.clone())
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
	async fn record_usage_dispatch(
		&self,
		run: &Run,
		token: Uuid,
		dispatch: &UsageDispatch,
	) -> Result<()> {
		let _ = run;
		assert_eq!(token, self.0.token);
		self.record("usage.dispatch");
		let mut usage = self.0.usage.lock().unwrap();
		for (_, outcome) in usage.iter_mut().filter(|(_, outcome)| outcome.is_none()) {
			*outcome = Some(UsageOutcome::Unknown);
		}
		usage.push((dispatch.clone(), None));
		Ok(())
	}
	async fn calibration_samples(
		&self,
		upstream_model: &str,
		_projection_version: u8,
	) -> Result<Vec<aidash_domain::provider::estimate::CalibrationSample>> {
		assert_eq!(upstream_model, "fixture");
		self.record("calibration.samples");
		Ok(self.0.calibration.lock().unwrap().clone())
	}
	async fn complete_usage_record(
		&self,
		run: &Run,
		attempt: Uuid,
		outcome: &UsageOutcome,
	) -> Result<()> {
		let _ = run;
		self.record("usage.complete");
		if let Some((_, recorded @ None)) = self
			.0
			.usage
			.lock()
			.unwrap()
			.iter_mut()
			.find(|(dispatch, _)| dispatch.attempt == attempt)
		{
			*recorded = Some(outcome.clone());
		}
		Ok(())
	}
	async fn run_message_has_media(&self, messages: &[Uuid]) -> Result<bool> {
		let _ = messages;
		Ok(self.0.human_media_pending)
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
		let mut attempts = self.0.compaction_attempts.lock().unwrap();
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
			.compaction_attempts
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
		_staged: Option<&aidash_application::decision::StagedDecision>,
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
	async fn human_request(
		&self,
		run: &Run,
		kind: &str,
		prompt: &str,
		key: &str,
	) -> Result<HumanRequest> {
		let _ = key;
		self.record("home.human_request");
		let request = HumanRequest {
			id: Uuid::new_v4(),
			workspace_id: run.workspace_id,
			run_id: run.id,
			kind: kind.into(),
			prompt: prompt.into(),
			response: None,
			created_at: run.updated_at,
			answered_by: None,
		};
		*self.0.human.lock().unwrap() = Some(request.clone());
		Ok(request)
	}
	async fn human_request_by_id(&self, id: Uuid) -> Result<HumanRequest> {
		self.record("home.human_read");
		let request = self.0.human.lock().unwrap().clone().expect("Home request");
		assert_eq!(request.id, id);
		Ok(request)
	}
	async fn close_human_request(&self, id: Uuid, response: Value) -> Result<HumanRequest> {
		self.record("home.human_close");
		let mut human = self.0.human.lock().unwrap();
		let request = human.as_mut().expect("Home request");
		assert_eq!(request.id, id);
		if request.response.is_some() {
			return Err(Error::Conflict("human request already answered".into()));
		}
		request.response = Some(response);
		request.answered_by = Some("system".into());
		Ok(request.clone())
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
			if let Some(message) = self
				.0
				.messages
				.iter()
				.find(|message| message.id.to_string() == id)
			{
				return Ok(json!(message));
			}
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
				run_id: None,
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
		let _ = key;
		self.record("home.complete");
		self.0
			.artifacts
			.lock()
			.unwrap()
			.push(artifact.content.clone());
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
		self.0.local_authority || self.0.skill_tool
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
	async fn model_media(
		&self,
		selections: &[aidash_domain::media::ModelMedia],
	) -> Result<Vec<ContentPart>> {
		let _ = selections;
		unexpected("ExecutionAuthority.model_media")
	}
	async fn human_message_media(
		&self,
		messages: &[(i64, Uuid, usize)],
		model: &ModelConfig,
	) -> Result<HumanMediaBatch> {
		let _ = model;
		if !self.0.human_media_pending {
			unexpected("ExecutionAuthority.human_message_media");
		}
		Ok(HumanMediaBatch {
			parts: vec![],
			through_seq: messages.first().map(|(seq, _, _)| *seq),
			has_more: true,
		})
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
		Ok(self
			.0
			.summary_reservation
			.then(|| Box::new(self.clone()) as Box<dyn InferenceReservation>))
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
	async fn recheck_decider(&self, _: &aidash_domain::decision::DeciderPin) -> Result<()> {
		self.record("authority.decider_recheck");
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
	fn completion_checks(
		&self,
	) -> Option<&dyn aidash_application::ports::execution::completion::CompletionChecks> {
		self.0.checks.as_ref().map(|checks| {
			checks as &dyn aidash_application::ports::execution::completion::CompletionChecks
		})
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
		if let Some(text) = &self.0.skill_context {
			return Ok(text.clone());
		}
		if !self.0.skill_tool {
			unexpected("ExecutionEnvironment.skill_context")
		}
		self.record("source.skill");
		Ok(self.0.skill_text.lock().unwrap().clone())
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
	async fn skill_revision(&self, run: &Run) -> Result<Option<i64>> {
		let _ = run;
		Ok(*self.0.skill_revision.lock().unwrap())
	}
	async fn cache_scope(&self, run: &Run) -> Result<aidash_domain::projection::CacheScope> {
		let _ = run;
		if self.0.projection.is_legacy() {
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
		degrade_on_outage: bool,
	) -> Result<Option<Value>> {
		let _ = (run, task, inputs, budget, entry);
		self.0
			.semantic_keys
			.lock()
			.unwrap()
			.push(key.map(RetrievalKey::digest));
		self.0
			.semantic_degrade
			.lock()
			.unwrap()
			.push(degrade_on_outage);
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
	async fn url_intents(
		&self,
		_run: &Run,
	) -> Result<Vec<aidash_domain::research::intent::UrlIntent>> {
		Ok(self.0.url_intents.clone())
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
		if self.0.cancel_inference {
			return Ok(InferenceInterruption::Cancelled);
		}
		let mut signal = self.0.interruption.subscribe();
		let interruption = signal
			.wait_for(Option::is_some)
			.await
			.map(|value| value.expect("signalled interruption"))
			.expect("interruption sender lives with the backend");
		self.0.interruption.send_replace(None);
		Ok(interruption)
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
			skills: self.0.skills.clone(),
			max_steps: 64,
			allow_task_creation: None,
			conversation_memory: self.0.conversation_memory,
			context_policy: self.0.context_policy.clone(),
			tool_parallelism: 1,
			projection_version: self.0.projection,
			prompt_cache: self.0.prompt_cache,
			completion: self.0.completion.clone(),
		})
	}
	fn provider(&self, _model: ModelConfig) -> Result<Arc<dyn ModelProvider>> {
		Ok(Arc::new(self.clone()))
	}
	async fn compaction_decider(
		&self,
		run: &Run,
		_token: Uuid,
	) -> Result<Option<aidash_application::ports::decision::DecisionPorts<'_>>> {
		Ok(decider::ports(self, self.node_id(), run))
	}
	async fn summary_dependencies_current(
		&self,
		run: &Run,
		dependencies: &aidash_domain::context::summary::SummaryDependencies,
	) -> Result<bool> {
		let _ = run;
		self.record("summary.dependencies");
		let checks = self
			.0
			.calls
			.lock()
			.unwrap()
			.iter()
			.filter(|call| **call == "summary.dependencies")
			.count();
		// Past the threshold, revocation hits every set, or only the sets that
		// depend on `revoked_message` when one is named.
		let revoked = self
			.0
			.summary_checks_before_revocation
			.is_some_and(|current| checks > current)
			&& self
				.0
				.revoked_message
				.is_none_or(|id| dependencies.message_ids.contains(&id));
		Ok(!self.0.stale_summary_dependencies && !revoked)
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
		if self.0.stall_provider.load(Ordering::SeqCst) {
			std::future::pending::<()>().await;
		}
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
			output_tokens: if self.0.omit_output_usage { 0 } else { 10 },
			usage_complete: !self.0.omit_output_usage,
			reported: ReportedUsage {
				input_tokens: Some(self.0.reported_input_tokens.unwrap_or(200)),
				output_tokens: (!self.0.omit_output_usage).then_some(10),
				cache_read_tokens: Some(150),
				..Default::default()
			},
			..Default::default()
		})
	}
}
#[async_trait]
impl InferenceReservation for Backend {
	async fn settle(self: Box<Self>, response: &ModelResponse) -> Result<()> {
		assert_eq!(response.usage_complete, !self.0.omit_output_usage);
		self.record("reservation.settle");
		Ok(())
	}
	async fn abort(self: Box<Self>) -> Result<()> {
		self.record("reservation.abort");
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
		compaction_attempts: Mutex::new(vec![]),
		context_policy: None,
		overflows: Mutex::new(0),
		jev_retention: None,
		summary_text: Mutex::new(None),
		revoke_summarizer: false,
		stale_summary_dependencies: false,
		summary_checks_before_revocation: None,
		revoked_message: None,
		summary_reservation: false,
		terminal: None,
		summarizer_output_tokens: 4096,
		scripts: Mutex::new(VecDeque::new()),
		attempts: Mutex::new(vec![]),
		progress: Mutex::new(vec![]),
		interruption: tokio::sync::watch::channel(None).0,
		deny_recheck: false,
		fail_append: false,
		block_append: false,
		cancel_inference: false,
		usage: Mutex::new(vec![]),
		stall_provider: AtomicBool::new(false),
		omit_output_usage: false,
		tools: vec![],
		skills: vec![],
		skill_context: None,
		local_authority: false,
		messages: vec![],
		custom: BTreeMap::new(),
		invocations: Mutex::new(BTreeMap::new()),
		direct_skills: vec![],
		direct_bodies: BTreeMap::new(),
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
		semantic_degrade: Mutex::new(vec![]),
		inputs: Mutex::new(vec![]),
		instructions: "Do the task".into(),
		fill_semantic_budget: false,
		human_media_pending: false,
		prompt_cache: Default::default(),
		explicit_cache_model: false,
		known_evidence: vec![],
		research_revoked: false,
		research_revoked_after: None,
		research_checks: Default::default(),
		expired_research: Default::default(),
		artifacts: Mutex::new(vec![]),
		completion: None,
		checks: None,
		fenced: Mutex::new(vec![]),
		request_estimator: None,
		reported_input_tokens: None,
		calibration: Mutex::new(vec![]),
		url_intents: vec![],
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
				|| name.starts_with("usage.")
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
			"usage.dispatch",
			"authority.suspend",
			"visibility.suspend",
			"inference.start",
			"provider.infer",
			"authority.suspend",
			"visibility.resume",
			"usage.complete",
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
#[tokio::test]
async fn successful_inference_completes_its_usage_record(mut fixture: Fixture) {
	// Arrange
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	let usage = fixture.backend.0.usage.lock().unwrap();
	let [(dispatch, outcome)] = usage.as_slice() else {
		panic!("one Usage Record per attempt")
	};
	assert_ne!(dispatch.attempt, fixture.backend.0.token);
	assert_eq!(dispatch.lease, fixture.backend.0.token);
	assert_eq!(
		(dispatch.model_id.as_str(), dispatch.model_version.as_str()),
		("model", "1.0.0")
	);
	assert_eq!(dispatch.projection_version, 1);
	assert_eq!(dispatch.estimate.estimator, "bytes");
	assert_eq!(
		dispatch.estimate.confidence,
		EstimateConfidence::Conservative
	);
	let RunState::ToolCall(state) = &fixture.run.state else {
		panic!("inference must produce a tool-call state")
	};
	assert_eq!(dispatch.estimate.tokens, state.request_tokens as u64);
	assert_eq!(dispatch.response_epoch, state.response_epoch);
	assert_eq!(
		*outcome,
		Some(UsageOutcome::Completed(ReportedUsage {
			input_tokens: Some(200),
			output_tokens: Some(10),
			cache_read_tokens: Some(150),
			..Default::default()
		}))
	);
}
/// The durable context written after incomplete usage must stay readable by
/// workers whose `ContextUsage` decoder rejects unknown fields.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct PreviousContextUsage {
	input_tokens: u64,
	output_tokens: u64,
	context_window: usize,
	compactions: u32,
}
#[rstest]
#[tokio::test]
async fn incomplete_usage_keeps_the_durable_context_readable_by_older_workers(
	mut fixture: Fixture,
) {
	// Arrange
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.omit_output_usage = true;
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	let writes = fixture.backend.0.writes.lock().unwrap();
	let usage = serde_json::to_value(&writes.last().unwrap().1.context).unwrap()["usage"].clone();
	serde_json::from_value::<PreviousContextUsage>(usage)
		.expect("older workers decode the context usage");
	let recorded = fixture.backend.0.usage.lock().unwrap();
	let [(_, Some(UsageOutcome::Completed(reported)))] = recorded.as_slice() else {
		panic!("one completed Usage Record")
	};
	assert_eq!(reported.output_tokens, None);
}
#[rstest]
#[case::cancelled(true, None, UsageOutcome::Unknown)]
#[case::rejected(false, Some(503), UsageOutcome::Rejected { class: "provider_rejected".into() })]
#[tokio::test]
async fn failed_inference_records_an_outcome_without_counts(
	mut fixture: Fixture,
	#[case] cancel: bool,
	#[case] status: Option<u16>,
	#[case] expected: UsageOutcome,
) {
	// Arrange
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.cancel_inference = cancel;
	state.provider_status = status;
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	// Act
	let result = advance_sources(&mut fixture).await;
	// Assert
	if cancel {
		assert!(matches!(result, Err(Error::Conflict(_))));
	} else {
		assert!(matches!(result, Err(Error::ProviderRejected { .. })));
	}
	let usage = fixture.backend.0.usage.lock().unwrap();
	assert_eq!(usage.len(), 1);
	assert_eq!(usage[0].1, Some(expected));
	assert_eq!(fixture.run.phase(), RunPhase::Thinking);
}
#[rstest]
#[tokio::test]
async fn interrupted_attempt_becomes_unknown_when_the_run_dispatches_again(mut fixture: Fixture) {
	// Arrange: the worker stops while the provider call is in flight, as a
	// crash or lost lease would, so the first attempt stays dispatched.
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	fixture
		.backend
		.0
		.stall_provider
		.store(true, Ordering::SeqCst);
	{
		let backend = fixture.backend.clone();
		let mut visibility = backend.clone();
		let executor = Executor::new(&backend);
		let interrupted = executor.advance(&mut fixture.run, backend.0.token, &mut visibility);
		tokio::pin!(interrupted);
		tokio::select! {
			biased;
			_ = &mut interrupted => panic!("the provider call must stay in flight"),
			() = tokio::task::yield_now() => {}
		}
	}
	{
		let usage = fixture.backend.0.usage.lock().unwrap();
		assert_eq!(usage.len(), 1);
		assert_eq!(usage[0].1, None);
	}
	fixture
		.backend
		.0
		.stall_provider
		.store(false, Ordering::SeqCst);
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	let usage = fixture.backend.0.usage.lock().unwrap();
	assert_eq!(usage.len(), 2);
	assert_ne!(usage[0].0.attempt, usage[1].0.attempt);
	assert_eq!(usage[0].1, Some(UsageOutcome::Unknown));
	assert!(matches!(usage[1].1, Some(UsageOutcome::Completed(_))));
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
		let mut tools: Tools = self
			.0
			.tools
			.iter()
			.map(|name| ((*name).to_owned(), Advertised::builtin(name)))
			.collect();
		for (alias, tool) in &self.0.custom {
			tools.insert(alias.clone(), tool.clone());
		}
		if self.0.skill_tool {
			tools.insert("skill_list".into(), Arc::new(SkillList));
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
struct Observer;
#[async_trait]
impl ExecutionTool for Observer {
	fn specification(&self) -> aidash_domain::provider::ToolSpec {
		aidash_domain::provider::ToolSpec {
			name: "renamed_observer".into(),
			description: "Observe the workspace".into(),
			parameters: json!({"type":"object","properties":{}}),
		}
	}
	fn contract(&self) -> aidash_domain::tool::ToolContract {
		aidash_domain::tool::builtin_contract("workspace_observe").unwrap()
	}
	fn replay_safe(&self) -> bool {
		true
	}
	async fn invoke(&self, _: &Run, _: Value, _: &str) -> Result<Value> {
		unexpected("renamed_observer.invoke")
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
fn only_message_reads_among_workspace_records_are_summarizable() {
	let mut tools = Tools::new();
	tools.insert("renamed_reader".into(), Arc::new(RecordReader));
	tools.insert("renamed_observer".into(), Arc::new(Observer));
	let event = |name: &str, arguments: Value| {
		ContextEvent::tool(
			ToolCall {
				id: "call".into(),
				name: name.into(),
				arguments,
			},
			json!({"view":"workspace_observation_v1"}),
		)
	};
	let message = json!({"kind":"message","id":Uuid::from_u128(7)});
	assert!(summarizable(&event("renamed_reader", message), &tools));
	// Each of these records is filtered on its own, and a summary rechecks
	// only message sources, so the events stay verbatim.
	for kind in ["task", "artifact", "event", "workspace"] {
		let record = json!({"kind":kind,"id":Uuid::from_u128(8)});
		assert!(
			!summarizable(&event("renamed_reader", record), &tools),
			"{kind}"
		);
	}
	assert!(!summarizable(
		&event("renamed_observer", json!({"offset":0,"limit":20})),
		&tools
	));
	assert!(summarizable(&event("read", json!({"path":"a"})), &tools));
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
async fn provider_overflow_releases_its_reservation_before_the_reduced_retry(mut fixture: Fixture) {
	// Arrange
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = true;
	state.context_policy = Some(recovery_policy(false));
	*state.overflows.lock().unwrap() = 1;
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert: the rejected attempt's charge is released before the retry
	// state is saved, and its Usage Record stays a provider rejection.
	assert_eq!(events(&fixture).last().unwrap(), "run.context_overflow");
	let overflowed = calls(&fixture);
	assert!(position(&overflowed, "provider.infer") < position(&overflowed, "reservation.abort"));
	assert!(!overflowed.contains(&"reservation.settle"));
	assert_eq!(
		fixture.backend.0.usage.lock().unwrap()[0].1,
		Some(UsageOutcome::Rejected {
			class: "provider_rejected".into()
		})
	);
	// The reduced retry reserves once more and settles its own response.
	advance_sources(&mut fixture).await.unwrap();
	let calls = calls(&fixture);
	let count = |name| calls.iter().filter(|call| **call == name).count();
	assert_eq!(count("reservation.admit"), 2);
	assert_eq!(count("reservation.abort"), 1);
	assert_eq!(count("reservation.settle"), 1);
	assert!(matches!(fixture.run.state, RunState::ToolCall(_)));
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
		let attempts = fixture.backend.0.compaction_attempts.lock().unwrap();
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
		fixture.backend.0.compaction_attempts.lock().unwrap()[0].1,
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
	assert!(
		fixture
			.backend
			.0
			.compaction_attempts
			.lock()
			.unwrap()
			.is_empty()
	);
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
		fixture.backend.0.compaction_attempts.lock().unwrap()[0].1,
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
#[tokio::test]
async fn restored_originals_stay_paused_until_their_sources_are_readable(mut fixture: Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	*state.summary_text.lock().unwrap() = Some(valid_summary());
	thinking_with_history(&mut fixture, 40);
	advance_sources(&mut fixture).await.unwrap();
	let dependencies = fixture
		.run
		.context
		.execution_summary
		.as_ref()
		.unwrap()
		.dependencies
		.clone();
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.stale_summary_dependencies = true;
	assert!(matches!(
		advance_sources(&mut fixture).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		fixture.run.context.revoked_summary_dependencies,
		Some(dependencies)
	);

	// A resumed step finds no summary, yet the restored originals still
	// depend on the revoked sources: it pauses again before any provider.
	let earlier = calls(&fixture).len();
	let writes = events(&fixture).len();
	let requests = fixture.backend.0.requests.lock().unwrap().len();
	assert!(matches!(
		advance_sources(&mut fixture).await,
		Err(Error::Forbidden)
	));
	let step = &calls(&fixture)[earlier..];
	assert!(step.contains(&"summary.dependencies"), "{step:?}");
	for provider in ["jev.ask", "provider.infer", "summarizer.infer"] {
		assert!(!step.contains(&provider), "{provider}: {step:?}");
	}
	assert_eq!(fixture.backend.0.requests.lock().unwrap().len(), requests);
	assert_eq!(events(&fixture).len(), writes);

	// Once every source is readable again, the step reaches a provider and
	// the saved context no longer carries the guard.
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.stale_summary_dependencies = false;
	advance_sources(&mut fixture).await.unwrap();
	assert!(fixture.backend.0.requests.lock().unwrap().len() > requests);
	assert!(fixture.run.context.revoked_summary_dependencies.is_none());
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

// A trusted local Run releases no authority for Jev, yet its summary sources
// can still be deleted while pruning is in flight.
#[rstest]
#[case::scoped(true)]
#[case::local(false)]
#[tokio::test]
async fn a_summary_dependency_revoked_during_pruning_pauses_before_inference(
	mut fixture: Fixture,
	#[case] scoped: bool,
) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = scoped;
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	*state.summary_text.lock().unwrap() = Some(valid_summary());
	thinking_with_history(&mut fixture, 40);
	let originals = fixture.run.context.history.clone();
	advance_sources(&mut fixture).await.unwrap();
	assert!(fixture.run.context.execution_summary.is_some());

	// The check before pruning passes; the source is revoked before the
	// check that follows it.
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

// A trusted local Run has no authority guard, yet its summary sources can
// still be deleted while inference is in flight.
#[rstest]
#[case::scoped(true)]
#[case::local(false)]
#[tokio::test]
async fn a_summary_dependency_revoked_during_inference_discards_the_response(
	mut fixture: Fixture,
	#[case] scoped: bool,
) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = scoped;
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	*state.summary_text.lock().unwrap() = Some(valid_summary());
	thinking_with_history(&mut fixture, 40);
	let originals = fixture.run.context.history.clone();
	advance_sources(&mut fixture).await.unwrap();
	assert!(fixture.run.context.execution_summary.is_some());

	// Every check before inference, before and after pruning, passes; the
	// source is revoked while the ordinary model call is in flight.
	let before_inference = 2;
	let earlier = calls(&fixture).len();
	let checks = calls(&fixture)
		.iter()
		.filter(|call| **call == "summary.dependencies")
		.count();
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.summary_checks_before_revocation = Some(checks + before_inference);
	// The streamed text depends on the summary and is never stored.
	fixture.backend.script(vec![text("Completed task")]);
	assert!(matches!(
		advance_sources(&mut fixture).await,
		Err(Error::Forbidden)
	));
	assert!(fixture.backend.progress_text().is_empty());
	assert!(!calls(&fixture).contains(&"inference.append"));
	assert_eq!(
		fixture.backend.outcomes().last().unwrap(),
		&[ProgressOutcome::Interrupted(InterruptionReason::Revoked)]
	);

	let step = &calls(&fixture)[earlier..];
	let inferred = step.iter().position(|call| *call == "provider.infer");
	let last_check = step
		.iter()
		.rposition(|call| *call == "summary.dependencies");
	assert!(inferred.is_some() && inferred < last_check, "{step:?}");
	assert_eq!(
		step.iter()
			.filter(|call| **call == "summary.dependencies")
			.count(),
		before_inference + 1,
		"{step:?}"
	);
	// The response is never accepted: no tool step, and the originals are
	// restored in place of the revoked summary.
	assert!(matches!(fixture.run.state, RunState::Thinking(_)));
	assert_eq!(events(&fixture).last().unwrap(), "context.summary_revoked");
	assert!(fixture.run.context.execution_summary.is_none());
	assert_eq!(fixture.run.context.history, originals);
}

#[rstest]
#[case::scoped(true)]
#[case::local(false)]
#[tokio::test]
async fn summary_derived_progress_is_stored_only_once_its_sources_are_rechecked(
	mut fixture: Fixture,
	#[case] scoped: bool,
) {
	// Arrange
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = scoped;
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	*state.summary_text.lock().unwrap() = Some(valid_summary());
	thinking_with_history(&mut fixture, 40);
	advance_sources(&mut fixture).await.unwrap();
	assert!(fixture.run.context.execution_summary.is_some());
	let earlier = calls(&fixture).len();
	fixture.backend.script(vec![text("Completed task")]);
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert: nothing is stored while the provider streams; the held text is
	// stored after the post-response recheck, before acceptance.
	let step = &calls(&fixture)[earlier..];
	let last_check = step
		.iter()
		.rposition(|call| *call == "summary.dependencies")
		.unwrap();
	assert!(position(step, "provider.response") < last_check, "{step:?}");
	assert!(last_check < position(step, "inference.append"), "{step:?}");
	assert_eq!(fixture.backend.progress_text(), "Completed task");
	assert_eq!(
		fixture.backend.outcomes().last().unwrap(),
		&[ProgressOutcome::Accepted]
	);
	assert_eq!(fixture.backend.events().last().unwrap(), "model.completed");
}

#[rstest]
#[tokio::test]
async fn a_source_revoked_while_the_summarizer_runs_is_never_adopted(mut fixture: Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = true;
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	*state.summary_text.lock().unwrap() = Some(valid_summary());
	// No summary exists yet, so the checks are the ones before and after the
	// reservation and the one after the summarizer call, made once authority
	// resumes; the source is revoked during the call.
	state.summary_checks_before_revocation = Some(2);
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
		let attempts = fixture.backend.0.compaction_attempts.lock().unwrap();
		assert_eq!(attempts.len(), 1);
		assert_eq!(attempts[0].1, Some(Outcome::Unauthorized));
	}
	let calls = calls(&fixture);
	let summarized = calls.iter().position(|call| *call == "summarizer.infer");
	let last_check = calls
		.iter()
		.rposition(|call| *call == "summary.dependencies");
	assert!(summarized.is_some() && summarized < last_check, "{calls:?}");
	assert!(!calls.contains(&"provider.infer"));
}

#[rstest]
#[tokio::test]
async fn a_source_revoked_before_the_summarizer_call_is_never_disclosed(mut fixture: Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = true;
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	*state.summary_text.lock().unwrap() = Some(valid_summary());
	// The source is already revoked before the reservation: nothing is
	// charged and nothing is disclosed.
	state.summary_checks_before_revocation = Some(0);
	thinking_with_history(&mut fixture, 40);
	let before = fixture.run.context.history.clone();

	assert!(matches!(
		advance_sources(&mut fixture).await,
		Err(Error::Forbidden)
	));

	let calls = calls(&fixture);
	assert!(!calls.contains(&"summarizer.infer"), "{calls:?}");
	assert!(!calls.contains(&"authority.summary_reserve"), "{calls:?}");
	assert!(fixture.run.context.execution_summary.is_none());
	assert_eq!(fixture.run.context.history, before);
	let attempts = fixture.backend.0.compaction_attempts.lock().unwrap();
	assert_eq!(attempts.len(), 1);
	assert_eq!(attempts[0].1, Some(Outcome::Unauthorized));
}

#[rstest]
#[tokio::test]
async fn a_source_revoked_while_the_reservation_holds_releases_it(mut fixture: Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = true;
	state.summary_reservation = true;
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	*state.summary_text.lock().unwrap() = Some(valid_summary());
	// The check before the reservation passes; the source is revoked while
	// the reservation, which may wait on remote admission, is acquired.
	state.summary_checks_before_revocation = Some(1);
	thinking_with_history(&mut fixture, 40);
	let before = fixture.run.context.history.clone();

	assert!(matches!(
		advance_sources(&mut fixture).await,
		Err(Error::Forbidden)
	));

	let calls = calls(&fixture);
	assert!(!calls.contains(&"summarizer.infer"), "{calls:?}");
	assert!(!calls.contains(&"reservation.settle"), "{calls:?}");
	let position = |name| calls.iter().position(|call| *call == name).unwrap();
	assert!(position("authority.summary_reserve") < position("reservation.abort"));
	assert_eq!(fixture.run.context.history, before);
	let attempts = fixture.backend.0.compaction_attempts.lock().unwrap();
	assert_eq!(attempts.len(), 1);
	assert_eq!(attempts[0].1, Some(Outcome::Unauthorized));
}

#[rstest]
#[tokio::test]
async fn a_trusted_local_summary_rechecks_its_sources_after_the_call(mut fixture: Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	*state.summary_text.lock().unwrap() = Some(valid_summary());
	// No authority guard holds; the checks before the call pass and the
	// source is deleted while the summarizer call is in flight.
	state.summary_checks_before_revocation = Some(2);
	thinking_with_history(&mut fixture, 40);
	let before = fixture.run.context.history.clone();

	assert!(matches!(
		advance_sources(&mut fixture).await,
		Err(Error::Forbidden)
	));

	let calls = calls(&fixture);
	assert!(!calls.contains(&"authority.summary_reserve"), "{calls:?}");
	let summarized = calls.iter().position(|call| *call == "summarizer.infer");
	let last_check = calls
		.iter()
		.rposition(|call| *call == "summary.dependencies");
	assert!(summarized.is_some() && summarized < last_check, "{calls:?}");
	assert!(fixture.run.context.execution_summary.is_none());
	assert_eq!(fixture.run.context.history, before);
	assert!(!events(&fixture).contains(&"context.compacted".to_owned()));
	let attempts = fixture.backend.0.compaction_attempts.lock().unwrap();
	assert_eq!(attempts.len(), 1);
	assert_eq!(attempts[0].1, Some(Outcome::Unauthorized));
}

#[rstest]
#[tokio::test]
async fn a_carried_source_revoked_while_the_summarizer_runs_is_never_readopted(
	mut fixture: Fixture,
) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = true;
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	*state.summary_text.lock().unwrap() = Some(valid_summary());
	thinking_with_history(&mut fixture, 40);
	advance_sources(&mut fixture).await.unwrap();
	// The first summary read message `carried`; the next merge absorbs none.
	let carried = Uuid::from_u128(9);
	let summary = fixture.run.context.execution_summary.as_mut().unwrap();
	summary.dependencies.message_ids.insert(carried);
	let first = summary.digest.clone();
	thinking_with_history(&mut fixture, 40);
	// The checks before and after pruning and before and after the second
	// reservation pass; `carried` is revoked while the summarizer call is in
	// flight.
	let checks = calls(&fixture)
		.iter()
		.filter(|call| **call == "summary.dependencies")
		.count();
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.summary_checks_before_revocation = Some(checks + 4);
	state.revoked_message = Some(carried);
	let earlier = calls(&fixture).len();

	assert!(matches!(
		advance_sources(&mut fixture).await,
		Err(Error::Forbidden)
	));

	let step = &calls(&fixture)[earlier..];
	assert!(step.contains(&"summarizer.infer"), "{step:?}");
	assert_eq!(
		fixture
			.run
			.context
			.execution_summary
			.as_ref()
			.unwrap()
			.digest,
		first
	);
	let attempts = fixture.backend.0.compaction_attempts.lock().unwrap();
	assert_eq!(attempts.last().unwrap().1, Some(Outcome::Unauthorized));
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

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_blocked_progress_append_cannot_hold_a_completed_response(fixture: Fixture) {
	// Arrange
	let mut fixture = thinking(fixture);
	Arc::get_mut(&mut fixture.backend.0).unwrap().block_append = true;
	fixture.backend.script(vec![text("Completed task")]);
	let started = tokio::time::Instant::now();
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert: the response is accepted once the bounded append expires.
	assert!(started.elapsed() <= progress::APPEND_TIMEOUT + FLUSH_INTERVAL);
	assert_eq!(fixture.run.phase(), RunPhase::ToolCall);
	assert_eq!(fixture.backend.outcomes(), [[ProgressOutcome::Accepted]]);
	assert!(fixture.backend.progress_text().is_empty());
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
/// One unprocessed human run message.
fn run_message(fixture: &mut Fixture) {
	let id = Uuid::from_u128(0x20);
	let content = "Also include the rollback steps.";
	let workspace_id = fixture.run.workspace_id;
	let created_at = fixture.run.updated_at;
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.tools = default_tools();
	state.inputs = Mutex::new(vec![aidash_domain::run_input::RunInput {
		seq: 2,
		sender: "human".into(),
		content: content.into(),
		idempotency_key: "input-2".into(),
		message_id: Some(id),
		reference_only: false,
	}]);
	state.messages = vec![Message {
		id,
		workspace_id,
		sender: "human".into(),
		content: content.into(),
		idempotency_key: Some("input-2".into()),
		created_at,
		run_id: None,
	}];
	fixture.run.observed_input_seq = 1;
}
fn run_message_catch_up(fixture: &mut Fixture) {
	run_message(fixture);
	// An earlier summary turns every unprocessed run message into catch-up.
	fixture.run.context.run_message_summary = "The user wants a release checklist.".into();
	fixture.run.context.run_message_summary_seq = 1;
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

/// The step state a Legacy request carries.
fn step_state(request: &ModelRequest) -> Value {
	serde_json::to_value(request).unwrap()["context"]["current"].clone()
}

#[rstest]
#[tokio::test]
async fn url_intents_are_rendered_from_their_records_only_into_steps_that_carry_them(
	mut fixture: Fixture,
) {
	// Arrange: the Task asks for one page and a run message for another;
	// their text says nothing about pages.
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	run_message(&mut fixture);
	Arc::get_mut(&mut fixture.backend.0).unwrap().url_intents = vec![
		aidash_domain::research::intent::UrlIntent {
			input_key: None,
			pages: vec!["https://example.com/notes".into()],
		},
		aidash_domain::research::intent::UrlIntent {
			input_key: Some("input-2".into()),
			pages: vec!["https://example.org/rollback".into()],
		},
	];

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	let requests = fixture.backend.0.requests.lock().unwrap();
	let state = step_state(requests.last().unwrap());
	assert_eq!(
		state["url_intents"],
		json!({
			"instruction": aidash_domain::research::intent::URL_INTENT_INSTRUCTION,
			"pages": ["https://example.com/notes", "https://example.org/rollback"],
		})
	);
	assert_eq!(
		state["run_messages"][0]["read_pages"],
		json!(["https://example.org/rollback"])
	);
	assert_eq!(
		state["run_messages"][0]["content"], "Also include the rollback steps.",
		"the person's text is never rewritten"
	);
	assert_eq!(
		state["task"]["description"],
		fixture.backend.0.task.lock().unwrap().description
	);
}

#[rstest]
#[tokio::test]
async fn url_intents_of_summarized_run_messages_are_carried_forward(mut fixture: Fixture) {
	// Arrange: the run message that asked for a page was folded into the
	// run-message summary, so it is on no page of this step.
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	run_message(&mut fixture);
	fixture.run.context.run_message_summary = "Include the rollback steps.".into();
	fixture.run.context.run_message_summary_seq = 2;
	fixture.run.observed_input_seq = 2;
	Arc::get_mut(&mut fixture.backend.0).unwrap().url_intents =
		vec![aidash_domain::research::intent::UrlIntent {
			input_key: Some("input-2".into()),
			pages: vec!["https://example.org/rollback".into()],
		}];

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	let requests = fixture.backend.0.requests.lock().unwrap();
	let state = step_state(requests.last().unwrap());
	assert!(state.get("run_messages").is_none(), "{state}");
	assert_eq!(
		state["url_intents"]["pages"],
		json!(["https://example.org/rollback"])
	);
}

#[rstest]
#[tokio::test]
async fn url_intents_of_run_messages_taken_in_by_media_intake_are_carried_forward(
	mut fixture: Fixture,
) {
	// Arrange: a media-intake turn, which offers no tools, already took in
	// the run message that asked for a page; the summary does not cover it.
	fixture.run.state = RunState::Thinking(ThinkingState {
		media_intake_through_seq: Some(2),
		..ThinkingState::default()
	});
	run_message(&mut fixture);
	fixture.run.observed_input_seq = 2;
	Arc::get_mut(&mut fixture.backend.0).unwrap().url_intents =
		vec![aidash_domain::research::intent::UrlIntent {
			input_key: Some("input-2".into()),
			pages: vec!["https://example.org/rollback".into()],
		}];

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert: the page is still requested in the next actionable step.
	let requests = fixture.backend.0.requests.lock().unwrap();
	let state = step_state(requests.last().unwrap());
	assert!(state.get("run_messages").is_none(), "{state}");
	assert_eq!(
		state["url_intents"]["pages"],
		json!(["https://example.org/rollback"])
	);
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
	fn research(operation: aidash_domain::research::Operation) -> Arc<Self> {
		Arc::new(Self {
			specification: ToolSpec {
				name: aidash_domain::tool::providers::research_alias(operation).into(),
				description: "Research fixture".into(),
				parameters: json!({"type":"object"}),
			},
			contract: aidash_domain::tool::providers::research_contract(operation),
			outputs: Mutex::new(Default::default()),
			invocations: Mutex::new(0),
		})
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
			agent_plugins: vec![],
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
		saved.context.events().last(),
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
		saved.context.events().last(),
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
	// Usage measures only the tool definitions the catch-up request sent.
	let usage = fixture.run.context.usage.clone().unwrap().exposure.unwrap();
	assert_eq!(
		usage.schema_bytes,
		exposure::tool_bytes(&request.tools[0]).unwrap()
	);
	assert!(
		usage
			.exposed
			.iter()
			.all(|(alias, _)| alias == "workspace_read" || alias.starts_with("skill_")),
		"{:?}",
		usage.exposed
	);
}

#[rstest]
#[tokio::test]
async fn deferred_media_intake_reports_no_tool_exposure(mut fixture: Fixture) {
	// Arrange: an ordinary request whose human media intake continues.
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	run_message(&mut fixture);
	deferred_agent(&mut fixture);
	let backend = Arc::get_mut(&mut fixture.backend.0).unwrap();
	backend.human_media_pending = true;
	backend.scoped = true;
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert: no tools were sent, so none are measured as exposed.
	let requests = fixture.backend.0.requests.lock().unwrap();
	let [request] = requests.as_slice() else {
		panic!("expected one provider request, got {}", requests.len());
	};
	assert!(request.instructions.contains("Media intake is continuing"));
	assert!(request.tools.is_empty());
	let usage = fixture.run.context.usage.clone().unwrap().exposure.unwrap();
	assert_eq!(usage.schema_bytes, 0);
	let tools = mandatory_tools();
	assert!(
		usage
			.exposed
			.iter()
			.all(|(alias, _)| alias != "eager_tool" && !tools.contains(&alias.as_str())),
		"{:?}",
		usage.exposed
	);
}

#[rstest]
#[case::fits(100_000, "run.tool_recorded")]
#[case::deferred(4_000, "run.description_deferred")]
#[tokio::test]
async fn capability_descriptions_are_recorded_whole_or_deferred(
	mut fixture: Fixture,
	#[case] window: usize,
	#[case] expected: &str,
) {
	// Arrange: a description larger than a small remaining request window.
	deferred_agent(&mut fixture);
	let describe = Scripted::new(
		aidash_application::tools::builtins()
			.remove("capability_describe")
			.unwrap()
			.specification(),
		"capability_describe",
	);
	let description = json!({
		"alias": "tool_00", "kind": "tool", "digest": "sha256:tool",
		"detail": {"description": "z".repeat(40_000)}, "bytes": 40_000, "budget": 65_536,
	});
	*describe.outputs.lock().unwrap() = [description.clone()].into();
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.custom
		.insert("capability_describe".into(), describe);
	let request = call("call-0", "capability_describe", json!({"alias": "tool_00"}));
	respond(&mut fixture.run, vec![request.clone()]);
	fixture.run.state.tool_mut().unwrap().request_window = window;
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert: never a description the next request cannot carry.
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, expected);
	if let RunState::Thinking(thinking) = &saved.state {
		assert!(thinking.force_workspace_read_compaction);
		assert_eq!(thinking.deferred_skill_read.as_ref().unwrap().call, request);
		assert!(saved.context.history.is_empty());
	} else {
		assert!(matches!(
			saved.context.events().last(),
			Some(ContextEvent::Tool { call, result, .. })
				if call == &request && result == &description
		));
	}
}

#[rstest]
#[case::whole(100_000, Some(16))]
#[case::cut(4_000, None)]
#[case::deferred(1, Some(0))]
#[tokio::test]
async fn capability_search_pages_fit_the_remaining_request_budget(
	mut fixture: Fixture,
	#[case] window: usize,
	#[case] expected: Option<usize>,
) {
	// Arrange: a full page of capabilities with long localized names.
	deferred_agent(&mut fixture);
	let search = Scripted::new(
		aidash_application::tools::builtins()
			.remove("capability_search")
			.unwrap()
			.specification(),
		"capability_search",
	);
	let results = (0..16)
		.map(|index| {
			json!({"alias": format!("tool_{index:02}"), "kind": "tool",
				"name": "n".repeat(2_000), "description": "Fixture",
				"digest": "sha256:tool", "loaded": false})
		})
		.collect::<Vec<_>>();
	let page = json!({"results": results, "next_cursor": "32", "truncated": true});
	*search.outputs.lock().unwrap() = [page.clone()].into();
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.custom
		.insert("capability_search".into(), search);
	let request = call("call-0", "capability_search", json!({"cursor": "16"}));
	respond(&mut fixture.run, vec![request.clone()]);
	fixture.run.state.tool_mut().unwrap().request_window = window;
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert: never a page the next request cannot carry.
	let (event, saved) = last_write(&fixture);
	if expected == Some(0) {
		assert_eq!(event, "run.search_deferred");
		let RunState::Thinking(thinking) = &saved.state else {
			panic!("a deferred search returns to thinking");
		};
		assert!(thinking.force_workspace_read_compaction);
		assert_eq!(thinking.deferred_skill_read.as_ref().unwrap().call, request);
		assert!(saved.context.history.is_empty());
		return;
	}
	assert_eq!(event, "run.tool_recorded");
	let Some(ContextEvent::Tool { call, result, .. }) = saved.context.events().last() else {
		panic!("expected a tool result");
	};
	assert_eq!(call, &request);
	if expected == Some(16) {
		assert_eq!(result, &page);
		return;
	}
	// The page keeps its leading results and resumes at the first dropped one.
	let kept = result["results"].as_array().unwrap();
	assert!((1..16).contains(&kept.len()), "{}", kept.len());
	assert_eq!(kept[..], page["results"].as_array().unwrap()[..kept.len()]);
	assert_eq!(result["next_cursor"], json!((16 + kept.len()).to_string()));
	assert_eq!(result["truncated"], true);
	assert_eq!(result["budget_limited"], true);
}

#[rstest]
#[tokio::test]
async fn skill_asset_reads_fit_the_remaining_request_budget(mut fixture: Fixture) {
	// Arrange: a large asset page against a small remaining request window.
	deferred_agent(&mut fixture);
	let asset = Scripted::new(
		aidash_application::tools::builtins()
			.remove(SKILL_ASSET_READ)
			.unwrap()
			.specification(),
		SKILL_ASSET_READ,
	);
	let content = "z".repeat(40_000);
	*asset.outputs.lock().unwrap() = [json!({
		"alias": "guide", "path": "guide.md", "digest": "sha256:guide", "offset": 0,
		"content": content, "next_offset": null, "truncated": false,
	})]
	.into();
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.custom
		.insert(SKILL_ASSET_READ.into(), asset);
	respond(
		&mut fixture.run,
		vec![call(
			"call-0",
			SKILL_ASSET_READ,
			json!({"alias": "guide", "digest": "sha256:guide", "path": "guide.md"}),
		)],
	);
	let window = 4_000;
	fixture.run.state.tool_mut().unwrap().request_window = window;
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert: the page is cut to the headroom and continues from the cut.
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	let Some(ContextEvent::Tool { call, result, .. }) = saved.context.events().last() else {
		panic!("expected a tool result");
	};
	let page = result["content"].as_str().unwrap().len();
	assert!(page > 0 && page < content.len(), "{page}");
	assert_eq!(result["next_offset"], page);
	assert_eq!(result["budget_limited"], true);
	assert_eq!(call.arguments["max_chars"], page);
	assert!(saved.state.tool().unwrap().request_tokens <= window);
}

#[rstest]
#[tokio::test]
async fn compaction_leaves_the_exposure_set_intact(mut fixture: Fixture) {
	// Arrange: the Run's compaction Decider drops every unprotected call.
	Arc::get_mut(&mut fixture.backend.0).unwrap().jev_retention = Some(0.0);
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
		context.push(ContextEvent::tool(
			call(&format!("call-{index}"), "tool_05", json!({})),
			json!({"text": "z".repeat(4_000)}),
		));
	}
	context.journal.inferred_through = context.journal.head;
	let exposure = context.exposure.clone();
	let pinned = json!({});
	let tools = [];
	let mut budget = context::RequestBudget {
		window: usize::MAX,
		instructions: "Do the task",
		tools: &tools,
		max_output_tokens: 100,
		projection: &context::RequestProjection::Legacy,
		estimate: Default::default(),
	};
	budget.window = budget.request(&context, &pinned).estimated_total_tokens() / 2;
	let policy = context::policy::Effective::of(None);
	let fitting = aidash_application::context::Fitting {
		budget: &budget,
		pinned: &pinned,
		policy: &policy,
	};
	let ports = decider::ports(&fixture.backend, "aidash://fixture", &fixture.run);
	// Act
	let compaction = compact_decided_execution(
		&mut context,
		&fixture.run,
		fixture.backend.0.token,
		"aidash://fixture",
		ports.as_ref(),
		&fitting,
	)
	.await
	.unwrap();
	// Assert: the decision saved the pruned context, which ends the step.
	assert!(matches!(
		compaction,
		aidash_application::context::Compaction::Saved
	));
	assert_eq!(context.compactions, 1);
	assert!(context.history.len() < 24);
	assert_eq!(context.exposure, exposure);
}

mod batch;
mod decider;
mod degraded;
mod estimate;
mod native;
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
		ModelContext::Legacy(_) | ModelContext::Native(_) => panic!("expected an Ordered request"),
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
#[case::legacy(false, 1)]
#[case::ordered(true, 2)]
#[tokio::test]
async fn usage_record_carries_the_runs_pinned_projection_version(
	mut canonical: Fixture,
	#[case] is_ordered: bool,
	#[case] expected: u8,
) {
	// Arrange
	if is_ordered {
		ordered(&mut canonical);
	}
	// Act
	assert!(advance_sources(&mut canonical).await.is_err());
	// Assert
	let usage = canonical.backend.0.usage.lock().unwrap();
	let [(dispatch, _)] = usage.as_slice() else {
		panic!("one Usage Record per attempt")
	};
	assert_eq!(dispatch.projection_version, expected);
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
			estimate: Default::default(),
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
			estimate: Default::default(),
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

/// URL intents render beside the bounded snapshot, so the snapshot leaves
/// room for them rather than fitting alone and overflowing the request.
#[rstest]
#[tokio::test]
async fn url_intents_are_reserved_before_the_snapshot_is_bounded(mut canonical: Fixture) {
	let pages: Vec<String> = (0..5)
		.map(|n| format!("https://example.com/{n}/{}", "p".repeat(4000)))
		.collect();
	let state = Arc::get_mut(&mut canonical.backend.0).unwrap();
	// Leaves room for five long pages beside a snapshot bounded with them in
	// view, but not beside one bounded as if they were absent.
	state.instructions = "x".repeat(91_000);
	state.url_intents = vec![aidash_domain::research::intent::UrlIntent {
		input_key: None,
		pages: pages.clone(),
	}];
	canonical.backend.0.task.lock().unwrap().description = "d".repeat(400_000);
	assert!(matches!(
		advance_sources(&mut canonical).await,
		Err(Error::ProviderRejected { .. })
	));
	let requests = canonical.backend.0.requests.lock().unwrap();
	assert!(requests[0].ensure_fits(128_000).is_ok());
	assert!(
		requests[0]
			.input_body()
			.to_string()
			.contains("snapshot_truncated")
	);
	assert_eq!(
		step_state(&requests[0])["url_intents"]["pages"],
		json!(pages)
	);
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

mod research;

mod completion;
