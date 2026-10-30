use super::*;
use async_trait::async_trait;
use std::sync::{
	Mutex,
	atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn config(mode: Mode) -> DeciderConfig {
	DeciderConfig {
		hook: Hook::Compaction,
		answer_type: AnswerType::Noul,
		description: "Keep needed history".into(),
		provider_credential: "openrouter".into(),
		routes: vec![DeciderRoute::jev("typesafe/jev-1.13")],
		builder: BUILDER.into(),
		option_source: OPTION_SOURCE.into(),
		rule: RULE.into(),
		mode,
		max_calls_per_run: 1_000,
		state_retention: policy::StateRetention::default(),
	}
}
fn definition(mode: Mode) -> aidash_domain::registry::Entry {
	definition_of(&config(mode))
}
fn definition_of(config: &DeciderConfig) -> aidash_domain::registry::Entry {
	serde_json::from_value(json!({"id":"decider","version":"1.0.0","kind":"decider",
        "name":{"en":"Compaction"},"description":{"en":"Retain history"},"config":config}))
	.unwrap()
}
fn clef() -> DeciderRoute {
	DeciderRoute {
		provider_contract: DECISIONS.into(),
		model: "cloudflare/clef".into(),
		hosts: vec!["cloudflare".into(), "deepinfra".into()],
		threshold: Probability::new(0.3).unwrap(),
		state_budget: StateBudget {
			long_context_request_bytes: None,
			long_context_state_bytes: None,
			..StateBudget::default()
		},
	}
}
/// How a fixture Route answers a dispatched request.
#[derive(Clone, Copy)]
enum Fault {
	Transport,
	Invalid,
	Refused,
}
fn history() -> Context {
	let mut context = Context::default();
	for index in 0..10 {
		context.push(ContextEvent::Tool {
			call: aidash_domain::provider::ToolCall {
				id: format!("call{index}"),
				name: "safe_tool".into(),
				arguments: json!({"private":"INPUT_SECRET","query":"permitted"}),
			},
			result: json!({"body":format!("RESULT_SECRET{}", "x".repeat(10_000))}),
		});
	}
	// The decision state must never disclose the Execution Summary either.
	use aidash_domain::context::summary::*;
	let content = SummaryContent {
		goal: "PRIVATE_SUMMARY".into(),
		constraints: vec![],
		decisions: vec![],
		unresolved: vec![],
		resolved: vec![],
		artifacts: vec![],
		verification: vec![],
	};
	let provider = SummaryProvider {
		model: aidash_domain::registry::EntityRef {
			id: "summarizer".into(),
			version: "1.0.0".into(),
		},
		definition_digest: "digest".into(),
	};
	let summary = ExecutionSummary::merge(
		content,
		None,
		&context.history[..1],
		Default::default(),
		"context-recovery/1",
		provider,
	)
	.unwrap();
	context.execution_summary = Some(Box::new(summary));
	context
}
fn disclosure() -> Disclosure {
	Disclosure {
		goal: Some("Complete the permitted task".into()),
		tool_inputs: (0..10).map(|i| (i, json!({"query":"permitted"}))).collect(),
		sources: vec![SourcePin {
			resource: "authorized-history".into(),
			revision_digest: format!("sha256:{}", "a".repeat(64)),
		}],
		..Default::default()
	}
}
struct Saved {
	permits: Vec<DispatchPermit>,
	completed: BTreeMap<Uuid, (AttemptStatus, Option<BTreeMap<String, Probability>>)>,
	answers: usize,
	evidence: Vec<Evidence>,
	applied: usize,
	states: usize,
}
struct Fixture {
	events: usize,
	corrupt_boundary: bool,
	recovery_run: Option<Uuid>,
	run_revision: i64,
	plan_revision: usize,
	binding_restrictions: Restrictions,
	corrupt_bound_config: bool,
	omit_recovery: bool,
	revoke_in_reserve: bool,
	advance_on_reserve: Option<DateTime<Utc>>,
	clock: Mutex<Option<DateTime<Utc>>>,
	setup_error: Option<u8>,
	configuration_digest: String,
	provider_implementation: String,
	saved: Mutex<Saved>,
	budget: usize,
	fail_prepare: bool,
	forbid_initial: bool,
	forbid_after_initial: bool,
	deny_after_initial: bool,
	preparations: AtomicUsize,
	invalid: bool,
	invalid_response: bool,
	fail_transport: bool,
	fail_finish: bool,
	fail_finish_result: bool,
	fail_commit: bool,
	wrong_receipt: bool,
	revoke_after_dispatch: bool,
	invalid_final_approval: Option<Approval>,
	denied: AtomicBool,
	retain_state: bool,
	commit_time: Option<DateTime<Utc>>,
	expire_at_commit: bool,
	stricter_after_dispatch: bool,
	keep: Probability,
	keep_call: Option<Probability>,
	/// Per-question keep probabilities that override `keep`, by question id.
	answers: BTreeMap<String, Probability>,
	rule: &'static str,
	calls: AtomicUsize,
	checks: AtomicUsize,
	in_flight: AtomicUsize,
	peak: AtomicUsize,
	reads_denied: bool,
	routes: Vec<DeciderRoute>,
	faults: BTreeMap<usize, Fault>,
	/// The Route of every dispatched request, in order.
	dispatched_routes: Mutex<Vec<usize>>,
}
impl Fixture {
	/// A fixture whose Decider version lists `routes`.
	fn routed(mode: Mode, routes: Vec<DeciderRoute>) -> Self {
		let config = DeciderConfig {
			routes: routes.clone(),
			..config(mode)
		};
		Self {
			configuration_digest: config.digest().unwrap(),
			routes,
			..Self::new(mode)
		}
	}
	fn config(&self, mode: Mode) -> DeciderConfig {
		DeciderConfig {
			routes: self.routes.clone(),
			rule: self.rule.into(),
			..config(mode)
		}
	}
	/// A fixture whose Decider version names the opt-in `FIT_RULE` (#611).
	fn fit(mode: Mode, answers: BTreeMap<String, Probability>) -> Self {
		let config = DeciderConfig {
			rule: aidash_domain::decision::FIT_RULE.into(),
			..config(mode)
		};
		Self {
			configuration_digest: config.digest().unwrap(),
			rule: aidash_domain::decision::FIT_RULE,
			answers,
			..Self::new(mode)
		}
	}
	fn new(mode: Mode) -> Self {
		Self {
			events: 10,
			corrupt_boundary: false,
			recovery_run: None,
			run_revision: 1,
			plan_revision: 0,
			binding_restrictions: Restrictions::default(),
			corrupt_bound_config: false,
			omit_recovery: false,
			revoke_in_reserve: false,
			advance_on_reserve: None,
			clock: Mutex::new(None),
			setup_error: None,
			configuration_digest: config(mode).digest().unwrap(),
			provider_implementation: "node-decision-adapter:23".into(),
			saved: Mutex::new(Saved {
				permits: vec![],
				completed: BTreeMap::new(),
				answers: 0,
				evidence: vec![],
				applied: 0,
				states: 0,
			}),
			budget: usize::MAX,
			fail_prepare: false,
			forbid_initial: false,
			forbid_after_initial: false,
			deny_after_initial: false,
			preparations: AtomicUsize::new(0),
			invalid: false,
			invalid_response: false,
			fail_transport: false,
			fail_finish: false,
			fail_finish_result: false,
			fail_commit: false,
			wrong_receipt: false,
			revoke_after_dispatch: false,
			invalid_final_approval: None,
			denied: AtomicBool::new(false),
			retain_state: false,
			commit_time: None,
			expire_at_commit: false,
			stricter_after_dispatch: false,
			keep: Probability::new(0.0).unwrap(),
			keep_call: None,
			answers: BTreeMap::new(),
			rule: RULE,
			calls: AtomicUsize::new(0),
			checks: AtomicUsize::new(0),
			in_flight: AtomicUsize::new(0),
			peak: AtomicUsize::new(0),
			reads_denied: false,
			routes: config(mode).routes,
			faults: BTreeMap::new(),
			dispatched_routes: Mutex::new(vec![]),
		}
	}
	fn gate(&self) -> DecisionGate<'_> {
		DecisionGate {
			authority: self,
			journal: self,
			provider: self,
		}
	}
}
#[async_trait]
impl DecisionAuthority for Fixture {
	async fn check(&self, _: &Boundary, _: &DeciderPin, _: &[SourcePin]) -> Result<Approval> {
		let check = self.checks.fetch_add(1, Ordering::SeqCst);
		if self.setup_error == Some(0) && check > 0 {
			return Ok(Approval {
				restrictions: Restrictions {
					preserve_recent: 5,
					..Default::default()
				},
				state_retention: Default::default(),
			});
		}
		if self.denied.load(Ordering::SeqCst) || self.deny_after_initial && check > 0 {
			return Err(Error::Forbidden);
		}
		if let Some(approval) = &self.invalid_final_approval
			&& self.calls.load(Ordering::SeqCst) > 0
			&& self.in_flight.load(Ordering::SeqCst) == 0
		{
			return Ok(approval.clone());
		}
		Ok(Approval {
			restrictions: Restrictions {
				forbid_apply: self.forbid_initial || self.forbid_after_initial && check > 0,
				preserve_recent: if self.stricter_after_dispatch
					&& self.calls.load(Ordering::SeqCst) > 0
				{
					10
				} else {
					6
				},
				..Default::default()
			},
			state_retention: policy::StateRetention {
				enabled: self.retain_state,
				..Default::default()
			},
		})
	}
	async fn read(&self, _: &Evidence, _: bool) -> Result<()> {
		if self.reads_denied {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
}
#[async_trait]
impl DecisionJournal for Fixture {
	fn now(&self) -> DateTime<Utc> {
		self.clock.lock().unwrap().unwrap_or_else(|| {
			if self.calls.load(Ordering::SeqCst) > 0 && self.in_flight.load(Ordering::SeqCst) == 0 {
				self.commit_time.unwrap_or_else(Utc::now)
			} else {
				Utc::now()
			}
		})
	}
	async fn recover(&self, decision: Uuid, _: &Boundary) -> Result<Vec<RecoveredAttempt>> {
		if self.omit_recovery {
			return Ok(vec![]);
		}
		let saved = self.saved.lock().unwrap();
		if saved
			.evidence
			.iter()
			.any(|e| e.id == decision && e.outcome != Outcome::Rejected)
		{
			return Err(Error::Conflict(
				"decision already committed; reload Run".into(),
			));
		}
		Ok(saved
			.permits
			.iter()
			.filter(|p| p.record.decision == decision)
			.map(|permit| {
				let (status, answers) = saved
					.completed
					.get(&permit.record.attempt)
					.cloned()
					.unwrap_or((AttemptStatus::Uncertain, None));
				RecoveredAttempt {
					provider_report: Default::default(),
					permit: permit.clone(),
					status,
					answers,
				}
			})
			.collect())
	}
	async fn reserve(&self, record: &DispatchRecord) -> Result<Reservation> {
		assert!(self.preparations.load(Ordering::SeqCst) > 0);
		if let Some(kind) = self.setup_error {
			if kind == 1 {
				return Err(Error::Conflict("worker lease lost".into()));
			}
			if kind == 2 {
				return Err(Error::External("journal unavailable".into()));
			}
		}
		let mut saved = self.saved.lock().unwrap();
		if let Some(permit) = saved
			.permits
			.iter()
			.find(|p| p.record.attempt == record.attempt)
		{
			let (status, answers) = saved
				.completed
				.get(&record.attempt)
				.cloned()
				.unwrap_or((AttemptStatus::Uncertain, None));
			return Ok(Reservation::Recovered(RecoveredAttempt {
				provider_report: Default::default(),
				permit: permit.clone(),
				status,
				answers,
			}));
		}
		if saved.permits.len() >= self.budget {
			return Err(Error::Forbidden);
		}
		assert!(
			!saved
				.permits
				.iter()
				.any(|p| p.record.attempt == record.attempt)
		);
		let mut permit = DispatchPermit {
			record: record.clone(),
			owner_receipts: BTreeMap::from([
				("run-owner".into(), Uuid::new_v4()),
				("generation-ancestor".into(), Uuid::new_v4()),
			]),
		};
		if self.wrong_receipt {
			permit.record.request_digest = format!("sha256:{}", "b".repeat(64));
		}
		saved.permits.push(permit.clone());
		if self.revoke_in_reserve {
			self.denied.store(true, Ordering::SeqCst);
		}
		if let Some(now) = self.advance_on_reserve {
			*self.clock.lock().unwrap() = Some(now);
		}
		Ok(Reservation::Fresh(permit))
	}
	async fn finish_attempt(
		&self,
		permit: &DispatchPermit,
		answers: Option<&BTreeMap<String, Probability>>,
		status: AttemptStatus,
	) -> Result<()> {
		assert!(self.saved.lock().unwrap().permits.contains(permit));
		if self.fail_finish
			|| self.fail_finish_result
				&& permit
					.record
					.questions
					.iter()
					.any(|q| q.ends_with("_result"))
		{
			return Err(Error::External("journal failed".into()));
		}
		assert_eq!(answers.is_some(), status == AttemptStatus::Answered);
		self.saved
			.lock()
			.unwrap()
			.completed
			.insert(permit.record.attempt, (status, answers.cloned()));
		if answers.is_some() {
			self.saved.lock().unwrap().answers += 1;
		}
		Ok(())
	}
	async fn commit(
		&self,
		evidence: &Evidence,
		context: Option<&Context>,
		state: Option<&RetainedState>,
	) -> Result<()> {
		assert_eq!(context.is_some(), evidence.outcome == Outcome::Applied);
		if self.fail_commit {
			return Err(Error::Conflict("worker lease lost".into()));
		}
		if self.denied.load(Ordering::SeqCst) {
			assert_eq!(evidence.outcome, Outcome::Rejected);
			assert_eq!(evidence.reason, Reason::Forbidden);
			assert!(context.is_none() && state.is_none());
		}
		assert_eq!(
			state.is_some(),
			matches!(evidence.state, StateReference::Retained { .. })
		);
		if let Some(state) = state {
			assert_eq!(digest(&state.state), evidence.state_digest);
		}
		let mut committed = evidence.clone();
		let state = state.filter(|state| {
			let now = if self.expire_at_commit {
				state.expires_at
			} else {
				self.now()
			};
			if state.expires_at <= now {
				committed.state = StateReference::Expired {
					id: state.id,
					digest: state.digest.clone(),
					expired_at: state.expires_at,
				};
				false
			} else {
				true
			}
		});
		let mut saved = self.saved.lock().unwrap();
		saved.applied += usize::from(context.is_some());
		saved.states += usize::from(state.is_some());
		saved.evidence.push(committed);
		Ok(())
	}
	async fn commit_approval(&self, _: &ApprovalEvidence) -> Result<()> {
		unreachable!("compaction decisions commit no approval evidence")
	}
	async fn expire_states(&self, _: DateTime<Utc>) -> Result<usize> {
		Ok(0)
	}
	async fn append_outcome(&self, _: &OutcomeLink) -> Result<()> {
		Ok(())
	}
}
#[async_trait]
impl DecisionProvider for Fixture {
	fn implementation_id(&self) -> &str {
		&self.provider_implementation
	}
	fn configuration_digest(&self) -> Result<String> {
		Ok(self.configuration_digest.clone())
	}
	fn plan(&self, _: usize, state: &Value, questions: &Questions) -> Result<Vec<PreparedRequest>> {
		let serialized = state.to_string();
		for forbidden in [
			"PRIVATE_SUMMARY",
			"SYSTEM_SECRET",
			"DOCUMENT_SECRET",
			"INPUT_SECRET",
			"RESULT_SECRET",
		] {
			assert!(!serialized.contains(forbidden), "disclosed {forbidden}");
		}
		// The fixture provider's factual contract requires one question per request.
		Ok(questions
			.iter()
			.map(|(id, q)| PreparedRequest {
				body: serde_json::to_vec(
					&json!({"state":state,"question":q,"plan_revision":self.plan_revision}),
				)
				.unwrap(),
				questions: BTreeMap::from([(id.clone(), q.clone())]),
			})
			.collect())
	}
	async fn prepare(
		&self,
		route: usize,
		request: &PreparedRequest,
		_: Uuid,
	) -> Result<Box<dyn PreparedDispatch + '_>> {
		if self.fail_prepare {
			return Err(Error::Invalid("credential unavailable".into()));
		}
		self.preparations.fetch_add(1, Ordering::SeqCst);
		Ok(Box::new(FixtureDispatch {
			fixture: self,
			request: request.clone(),
			route,
		}))
	}
}
struct FixtureDispatch<'a> {
	fixture: &'a Fixture,
	request: PreparedRequest,
	route: usize,
}
#[async_trait]
impl PreparedDispatch for FixtureDispatch<'_> {
	async fn dispatch(
		self: Box<Self>,
	) -> std::result::Result<BTreeMap<String, Probability>, DispatchError> {
		let fixture = self.fixture;
		let request = &self.request;
		// Assert reservation-before-I/O and exact physical request digest.
		assert!(
			fixture
				.saved
				.lock()
				.unwrap()
				.permits
				.iter()
				.any(|p| p.record.request_digest == request.digest())
		);
		fixture.calls.fetch_add(1, Ordering::SeqCst);
		let concurrent = fixture.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
		fixture.peak.fetch_max(concurrent, Ordering::SeqCst);
		tokio::task::yield_now().await;
		fixture.in_flight.fetch_sub(1, Ordering::SeqCst);
		if fixture.revoke_after_dispatch {
			fixture.denied.store(true, Ordering::SeqCst);
		}
		fixture.dispatched_routes.lock().unwrap().push(self.route);
		match fixture.faults.get(&self.route) {
			Some(Fault::Transport) => {
				return Err(Error::External("route outage".into()).into());
			}
			Some(Fault::Invalid) => return Err(DispatchError::InvalidAnswers),
			Some(Fault::Refused) => return Err(DispatchError::Refused(402)),
			None => {}
		}
		if fixture.fail_transport {
			return Err(Error::External("RAW_PRIVATE_PROVIDER_BODY".into()).into());
		}
		if fixture.invalid_response {
			return Err(DispatchError::InvalidAnswers);
		}
		if fixture.invalid {
			return Ok(BTreeMap::new());
		}
		Ok(request
			.questions
			.keys()
			.map(|id| {
				(
					id.clone(),
					if let Some(answer) = fixture.answers.get(id) {
						*answer
					} else if id.ends_with("_call") {
						fixture.keep_call.unwrap_or(fixture.keep)
					} else {
						fixture.keep
					},
				)
			})
			.collect())
	}
}
async fn evaluate(
	fixture: &Fixture,
	mode: Mode,
	window: usize,
	view: Disclosure,
) -> (Result<CompactionResult>, Context, Context) {
	let mut context = history();
	let template = context.history.clone();
	context.history.clear();
	context.journal = Default::default();
	for i in 0..fixture.events {
		let mut event = template[i % template.len()].event.clone();
		if let ContextEvent::Tool { call, .. } = &mut event {
			call.id = format!("call{i}");
		}
		context.push(event);
	}
	let original = context.clone();
	let pinned = json!({"reference_documents":"DOCUMENT_SECRET"});
	let budget = RequestBudget {
		input_format: Default::default(),
		window,
		instructions: "SYSTEM_SECRET",
		tools: &[],
		max_output_tokens: 0,
		projection: &aidash_domain::context::RequestProjection::Legacy,
		estimate: Default::default(),
	};
	let mut boundary = Boundary {
		node: "aidash://execution".into(),
		run: fixture.recovery_run.unwrap_or_else(Uuid::new_v4),
		step: 1,
		run_revision: fixture.run_revision,
		input_revision: 2,
		worker: Uuid::new_v4(),
		input_digest: compaction_input_digest(&context, &budget, &pinned).unwrap(),
		cycle: 0,
		retry: 0,
	};
	if fixture.corrupt_boundary {
		boundary.input_digest = format!("sha256:{}", "b".repeat(64));
	}
	let config = fixture.config(mode);
	let definition = definition_of(&config);
	let pin = DeciderPin {
		identity: aidash_domain::registry::bindings::QualifiedRef {
			registry_deployment: boundary.node.clone(),
			id: definition.id.clone(),
			version: definition.version.clone(),
		},
		definition_digest: digest(&serde_json::to_value(&definition).unwrap()),
		configuration_digest: config.digest().unwrap(),
		provider_implementation: "node-decision-adapter:23".into(),
	};
	let mut bound = BoundDecider {
		pin,
		config,
		restrictions: fixture.binding_restrictions.clone(),
	};
	if fixture.corrupt_bound_config {
		bound.config.routes[0].model = "typesafe/jev-1.14".into();
	}
	let input = Evaluation {
		boundary: &boundary,
		decider: &bound,
		definition: &definition,
		disclosure: &view,
		now: Utc::now(),
	};
	let result = fixture
		.gate()
		.compact(&mut context, &budget, &pinned, &input)
		.await;
	(result, context, original)
}
#[tokio::test]
async fn fitting_context_needs_no_authority_provider_or_allowance() {
	let fixture = Fixture {
		budget: 0,
		denied: AtomicBool::new(true),
		..Fixture::new(Mode::Enforce)
	};
	let (result, context, original) =
		evaluate(&fixture, Mode::Enforce, 200_000, disclosure()).await;
	assert!(matches!(result.unwrap(), CompactionResult::AlreadyFits));
	assert_eq!(
		serde_json::to_value(context).unwrap(),
		serde_json::to_value(original).unwrap()
	);
	assert_eq!(fixture.checks.load(Ordering::SeqCst), 0);
	assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn all_batches_are_reserved_and_evidenced_before_atomic_apply_without_a_concurrency_cap() {
	let fixture = Fixture::new(Mode::Enforce);
	let (result, context, original) = evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
	assert!(matches!(result.unwrap(), CompactionResult::Applied(_)));
	assert_eq!(context.history.len(), 7);
	assert_eq!(context.history[0], original.history[0]);
	assert_eq!(&context.history[1..], &original.history[4..]);
	assert_eq!(fixture.calls.load(Ordering::SeqCst), 6);
	assert!(fixture.peak.load(Ordering::SeqCst) > 4);
	let saved = fixture.saved.lock().unwrap();
	assert_eq!(saved.applied, 1);
	assert_eq!(saved.states, 0);
	assert_eq!(saved.answers, 6);
	let evidence = &saved.evidence[0];
	assert_eq!(evidence.replay().unwrap(), evidence.branches);
	assert_eq!(evidence.fit.dropped, 3);
	let safe = serde_json::to_string(&evidence.summary()).unwrap();
	assert!(!safe.contains("safe_tool"));
	assert!(!safe.contains("authorized-history"));
}
#[tokio::test]
async fn an_unavailable_route_hands_the_decision_to_the_next_route() {
	for fault in [Fault::Transport, Fault::Invalid] {
		let fixture = Fixture {
			faults: BTreeMap::from([(0, fault)]),
			..Fixture::routed(
				Mode::Enforce,
				vec![DeciderRoute::jev("typesafe/jev-1.13"), clef()],
			)
		};
		let (result, context, _) = evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
		assert!(matches!(result.unwrap(), CompactionResult::Applied(_)));
		assert_eq!(context.history.len(), 7);
		let routes = fixture.dispatched_routes.lock().unwrap().clone();
		assert_eq!(routes, [[0; 6], [1; 6]].concat());
		let saved = fixture.saved.lock().unwrap();
		// The failed Route keeps its own rejected evidence under its own decision.
		let [failed, answered] = saved.evidence.as_slice() else {
			panic!("one evidence per Route");
		};
		assert_eq!((failed.route, failed.outcome), (0, Outcome::Rejected));
		assert_ne!(failed.id, answered.id);
		assert_eq!(answered.route, 1);
		assert_eq!(answered.model, "cloudflare/clef");
		assert_eq!(answered.provider_contract, DECISIONS);
		assert_eq!(answered.outcome, Outcome::Applied);
		// The Route's own threshold applies, under the Binding's.
		assert_eq!(answered.restrictions.threshold.value(), 0.3);
		assert_eq!(answered.replay().unwrap(), answered.branches);
		// The pin commits to each Route's threshold, so evidence cannot claim a
		// looser one than the Route it asked, nor rewrite the Route's.
		let mut looser = answered.clone();
		looser.restrictions.threshold = Probability::half();
		assert!(matches!(
			looser.replay(),
			Err(aidash_domain::Error::Invalid(message)) if message.contains("Route asked")
		));
		looser.route_thresholds[1] = Probability::half();
		assert!(looser.replay().is_err());
		failed.validate_rejection().unwrap();
	}
}
#[tokio::test]
async fn a_route_that_refuses_the_state_records_evidence_before_the_next_route() {
	// The rubric alone is larger than this budget's state.
	let budget = StateBudget {
		default_request_bytes: 200,
		default_state_bytes: 100,
		long_context_request_bytes: None,
		long_context_state_bytes: None,
		input_stages: vec![Some(20)],
	};
	let narrow = DeciderRoute {
		state_budget: budget.clone(),
		..DeciderRoute::jev("typesafe/jev-1.13")
	};
	let fixture = Fixture::routed(Mode::Enforce, vec![narrow.clone(), clef()]);
	let (result, context, _) = evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
	assert!(matches!(result.unwrap(), CompactionResult::Applied(_)));
	assert_eq!(context.history.len(), 7);
	assert!(
		fixture
			.dispatched_routes
			.lock()
			.unwrap()
			.iter()
			.all(|route| *route == 1)
	);
	{
		let saved = fixture.saved.lock().unwrap();
		let [refused, answered] = saved.evidence.as_slice() else {
			panic!("one evidence per Route");
		};
		assert_eq!(
			(refused.route, refused.reason),
			(0, Reason::ProviderFailure)
		);
		assert!(refused.attempts.is_empty());
		refused.validate_rejection().unwrap();
		assert_eq!((answered.route, answered.outcome), (1, Outcome::Applied));
	}
	// When no Route accepts the state, the Run pauses at once, and each
	// Route's refusal is on record.
	let fixture = Fixture::routed(
		Mode::Enforce,
		vec![
			narrow,
			DeciderRoute {
				state_budget: budget,
				..clef()
			},
		],
	);
	let (result, context, original) = evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
	assert!(matches!(
		result,
		Err(Error::Context(
			aidash_domain::context::recovery::Failure::DeciderUnavailable
		))
	));
	assert_eq!(context.history, original.history);
	assert!(fixture.dispatched_routes.lock().unwrap().is_empty());
	let routes: Vec<_> = (fixture.saved.lock().unwrap().evidence.iter())
		.map(|e| (e.route, e.reason))
		.collect();
	assert_eq!(
		routes,
		[(0, Reason::ProviderFailure), (1, Reason::ProviderFailure)]
	);
}
#[tokio::test]
async fn a_refusal_pauses_without_asking_another_route() {
	let fixture = Fixture {
		faults: BTreeMap::from([(0, Fault::Refused)]),
		..Fixture::routed(
			Mode::Enforce,
			vec![DeciderRoute::jev("typesafe/jev-1.13"), clef()],
		)
	};
	let (result, context, original) = evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
	assert!(!matches!(result, Ok(_) | Err(Error::DeciderOutage)));
	assert_eq!(context.history, original.history);
	assert!(
		fixture
			.dispatched_routes
			.lock()
			.unwrap()
			.iter()
			.all(|route| *route == 0)
	);
	let refused = fixture.saved.lock().unwrap().evidence.clone();
	assert_eq!(refused.len(), 1);
	assert_eq!(refused[0].reason, Reason::ProviderFailure);
	refused[0].validate_rejection().unwrap();
}
#[tokio::test]
async fn every_route_unavailable_is_an_outage_the_step_retries() {
	let fixture = Fixture {
		faults: BTreeMap::from([(0, Fault::Transport), (1, Fault::Invalid)]),
		..Fixture::routed(
			Mode::Enforce,
			vec![DeciderRoute::jev("typesafe/jev-1.13"), clef()],
		)
	};
	let (result, context, original) = evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
	assert!(matches!(result, Err(Error::DeciderOutage)));
	assert_eq!(context.history, original.history);
	let routes: Vec<_> = (fixture.saved.lock().unwrap().evidence.iter())
		.map(|e| e.route)
		.collect();
	assert_eq!(routes, [0, 1]);
	// A single unavailable Route is an outage too.
	let fixture = Fixture {
		faults: BTreeMap::from([(0, Fault::Transport)]),
		..Fixture::new(Mode::Enforce)
	};
	let (result, _, _) = evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
	assert!(matches!(result, Err(Error::DeciderOutage)));
}
#[tokio::test]
async fn shadow_records_the_proposal_and_never_mutates_context() {
	let fixture = Fixture::new(Mode::Shadow);
	let (result, context, original) = evaluate(&fixture, Mode::Shadow, 80_000, disclosure()).await;
	assert!(matches!(result.unwrap(), CompactionResult::Shadow(_)));
	assert_eq!(
		serde_json::to_value(context).unwrap(),
		serde_json::to_value(original).unwrap()
	);
	let saved = fixture.saved.lock().unwrap();
	assert_eq!(saved.applied, 0);
	assert_eq!(saved.evidence[0].outcome, Outcome::Shadow);
}
#[tokio::test]
async fn provider_batches_exceed_sixteen_without_a_fixed_fanout_ceiling() {
	let fixture = Fixture {
		events: 60,
		..Fixture::new(Mode::Enforce)
	};
	let mut view = disclosure();
	view.tool_inputs = (0..60).map(|i| (i, json!({"query":"permitted"}))).collect();
	evaluate(&fixture, Mode::Enforce, 80_000, view)
		.await
		.0
		.unwrap();
	assert_eq!(fixture.calls.load(Ordering::SeqCst), 106);
	assert!(fixture.peak.load(Ordering::SeqCst) > 16);
	assert_eq!(
		fixture.saved.lock().unwrap().evidence[0].attempts.len(),
		106
	);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_workers_share_durable_allowance_without_overspending_or_partial_apply() {
	let fixture = std::sync::Arc::new(Fixture {
		budget: 6,
		..Fixture::new(Mode::Enforce)
	});
	let first = fixture.clone();
	let second = fixture.clone();
	let first =
		tokio::spawn(async move { evaluate(&first, Mode::Enforce, 80_000, disclosure()).await });
	let second =
		tokio::spawn(async move { evaluate(&second, Mode::Enforce, 80_000, disclosure()).await });
	for (result, context, original) in [first.await.unwrap(), second.await.unwrap()] {
		match result {
			Ok(CompactionResult::Applied(_)) => assert_eq!(context.history.len(), 7),
			Err(_) => assert_eq!(context.history, original.history),
			other => panic!("unexpected concurrent decision outcome: {other:?}"),
		}
	}
	let saved = fixture.saved.lock().unwrap();
	assert_eq!(saved.permits.len(), 6);
	assert_eq!(fixture.calls.load(Ordering::SeqCst), 6);
	assert!(saved.applied <= 1);
	assert_eq!(saved.evidence.len(), 2);
}
#[tokio::test]
async fn changed_input_or_provider_pin_never_reaches_dispatch() {
	for mode in [Mode::Enforce, Mode::Shadow] {
		for kind in 0..3 {
			let mut fixture = Fixture {
				corrupt_boundary: kind == 0,
				..Fixture::new(mode)
			};
			if kind == 1 {
				fixture.configuration_digest = format!("sha256:{}", "c".repeat(64));
			}
			if kind == 2 {
				fixture.provider_implementation = "node-decision-adapter:24".into();
				assert_eq!(fixture.configuration_digest, config(mode).digest().unwrap());
			}
			let (result, context, original) = evaluate(&fixture, mode, 80_000, disclosure()).await;
			assert!(result.is_err(), "changed pin kind {kind} in {mode:?}");
			assert_eq!(context.history, original.history);
			assert_eq!(fixture.checks.load(Ordering::SeqCst), 0);
			assert_eq!(fixture.preparations.load(Ordering::SeqCst), 0);
			assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
			assert!(fixture.saved.lock().unwrap().permits.is_empty());
		}
	}
}

#[tokio::test]
async fn failed_transport_preparation_never_reserves_a_provider_call() {
	for mode in [Mode::Enforce, Mode::Shadow] {
		let fixture = Fixture {
			fail_prepare: true,
			..Fixture::new(mode)
		};
		let (result, context, original) = evaluate(&fixture, mode, 80_000, disclosure()).await;
		assert!(matches!(result, Err(Error::Invalid(_))));
		assert_eq!(context.history, original.history);
		assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
		let saved = fixture.saved.lock().unwrap();
		assert!(saved.permits.is_empty());
		assert_eq!(saved.applied, 0);
		assert_eq!(saved.evidence.len(), 1);
		assert_eq!(saved.evidence[0].reason, Reason::ProviderFailure);
		assert!(saved.evidence[0].attempts.is_empty());
	}
}

#[tokio::test]
async fn denied_batch_authority_or_allowance_returns_forbidden_after_evidencing_siblings() {
	for mode in [Mode::Enforce, Mode::Shadow] {
		for (deny_after_initial, budget) in [(true, usize::MAX), (false, 0), (false, 2)] {
			let fixture = Fixture {
				deny_after_initial,
				budget,
				..Fixture::new(mode)
			};
			let (result, context, original) = evaluate(&fixture, mode, 80_000, disclosure()).await;
			assert!(matches!(result, Err(Error::Forbidden)));
			assert_eq!(context.history, original.history);
			let saved = fixture.saved.lock().unwrap();
			let dispatched = if deny_after_initial { 0 } else { budget };
			assert_eq!(fixture.calls.load(Ordering::SeqCst), dispatched);
			assert_eq!(saved.permits.len(), dispatched);
			assert_eq!(saved.applied, 0);
			assert_eq!(saved.states, 0);
			assert_eq!(saved.evidence.len(), 1);
			let evidence = &saved.evidence[0];
			assert_eq!(evidence.outcome, Outcome::Rejected);
			assert_eq!(evidence.reason, Reason::Forbidden);
			assert_eq!(evidence.attempts.len(), dispatched);
			assert_eq!(evidence.answers.len(), dispatched);
			for (attempt, permit) in evidence.attempts.iter().zip(&saved.permits) {
				assert_eq!(attempt.id, permit.record.attempt);
				assert_eq!(attempt.owner_receipts, permit.owner_receipts);
			}
		}
	}
}

#[tokio::test]
async fn forbid_apply_policy_changes_are_forbidden_and_shadow_remains_a_proposal() {
	for forbid_initial in [false, true] {
		for mode in [Mode::Enforce, Mode::Shadow] {
			let fixture = Fixture {
				forbid_initial,
				forbid_after_initial: !forbid_initial,
				..Fixture::new(mode)
			};
			let (result, context, original) = evaluate(&fixture, mode, 80_000, disclosure()).await;
			assert_eq!(context.history, original.history);
			let saved = fixture.saved.lock().unwrap();
			assert_eq!(saved.applied, 0);
			let evidence = &saved.evidence[0];
			assert_eq!(evidence.reason, Reason::Forbidden);
			assert!(evidence.restrictions.forbid_apply);
			if mode == Mode::Enforce {
				assert!(matches!(result, Err(Error::Forbidden)));
				assert_eq!(evidence.outcome, Outcome::Rejected);
				assert!(saved.permits.is_empty());
				assert!(evidence.attempts.is_empty());
				assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
			} else {
				assert!(matches!(result, Ok(CompactionResult::Shadow(_))));
				assert_eq!(evidence.outcome, Outcome::Shadow);
				assert_eq!(saved.permits.len(), 6);
				assert_eq!(evidence.attempts.len(), 6);
				assert_eq!(evidence.replay().unwrap(), evidence.branches);
			}
		}
	}
}
/// A rejection that stopped before branch selection reports no branch and the
/// context that did not fit, unchanged; one without candidates asked nothing.
#[tokio::test]
async fn early_rejections_carry_no_branch_results_or_unasked_questions() {
	let fixture = Fixture {
		fail_prepare: true,
		..Fixture::new(Mode::Enforce)
	};
	let _ = evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
	let failed = fixture.saved.lock().unwrap().evidence[0].clone();
	assert_eq!(failed.reason, Reason::ProviderFailure);
	failed.validate_rejection().unwrap();
	let complete = Fixture::new(Mode::Enforce);
	evaluate(&complete, Mode::Enforce, 80_000, disclosure())
		.await
		.0
		.unwrap();
	let applied = complete.saved.lock().unwrap().evidence[0].clone();
	assert!(!applied.branches.is_empty());
	let mut premature = failed.clone();
	premature.branches = applied.branches.clone();
	let mut shrunk = failed.clone();
	shrunk.fit.retained -= 1;
	shrunk.fit.dropped += 1;
	let mut proposed = failed.clone();
	proposed.fit.proposed = proposed.fit.window;
	let mut relabeled = failed.clone();
	relabeled.reason = Reason::NoCandidates;
	// Candidates an early rejection reports are still the builder's.
	assert!(!failed.candidates.is_empty());
	let mut misplaced = failed.clone();
	misplaced.candidates[0].history_index = misplaced.history_length;
	let mut renamed = failed.clone();
	renamed.candidates[0].id = "other".into();
	let mut shared = failed.clone();
	shared.candidates[0].keep_result = shared.candidates[0].keep_call.clone();
	let mut unasked = failed.clone();
	unasked.questions.pop_first();
	for (case, corrupt) in [
		("premature branches", premature),
		("dropped history", shrunk),
		("proposed context", proposed),
		("asked without candidates", relabeled),
		("candidate outside the history", misplaced),
		("candidate under another ID", renamed),
		("candidate questions shared", shared),
		("question without its candidate", unasked),
	] {
		assert!(corrupt.validate_rejection().is_err(), "{case}");
	}
	let protected = Fixture::new(Mode::Enforce);
	let _ = evaluate(&protected, Mode::Enforce, 80_000, Disclosure::default()).await;
	let none = protected.saved.lock().unwrap().evidence[0].clone();
	assert_eq!(none.reason, Reason::NoCandidates);
	none.validate_rejection().unwrap();
}

#[tokio::test]
async fn history_without_disclosure_proof_stays_protected_with_zero_attempts() {
	let fixture = Fixture::new(Mode::Enforce);
	let (result, context, original) =
		evaluate(&fixture, Mode::Enforce, 80_000, Disclosure::default()).await;
	// Nothing could be classified, so pruning changes nothing and the Summary
	// Stage may still reduce the context.
	assert!(matches!(result, Ok(CompactionResult::Unpruned(_))));
	assert_eq!(context.history, original.history);
	assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
	assert_eq!(
		fixture.saved.lock().unwrap().evidence[0].reason,
		Reason::NoCandidates
	);
}
#[tokio::test]
async fn call_only_branch_keeps_call_and_historical_result_prefix() {
	let fixture = Fixture {
		keep_call: Some(Probability::half()),
		..Fixture::new(Mode::Enforce)
	};
	let (result, context, original) = evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
	result.unwrap();
	assert_eq!(context.history.len(), 10);
	let ContextEvent::Tool { call: before, .. } = &original.history[1].event else {
		panic!()
	};
	let ContextEvent::Tool {
		call: after,
		result,
	} = &context.history[1].event
	else {
		panic!()
	};
	assert_eq!(before, after);
	assert!(
		result
			.as_str()
			.unwrap()
			.contains("call the tool again only if the rest is still needed verbatim")
	);
	let saved = fixture.saved.lock().unwrap();
	assert_eq!(saved.evidence[0].fit.truncated, 3);
	assert!(
		saved.evidence[0]
			.replay()
			.unwrap()
			.values()
			.all(|b| *b == Branch::TruncateResult)
	);
	for truncated in 0..=3 {
		let mut recovered = saved.evidence[0].clone();
		recovered.fit.truncated = truncated;
		assert_eq!(recovered.replay().unwrap(), recovered.branches);
	}
	let mut corrupt = saved.evidence[0].clone();
	corrupt.fit.truncated = 4;
	assert!(corrupt.replay().is_err());
	assert!(
		!serde_json::to_string(&saved.evidence)
			.unwrap()
			.contains("RESULT_SECRET")
	);
}
#[tokio::test]
async fn failed_invalid_incomplete_or_unpersisted_answers_preserve_original_context() {
	for kind in 0..5 {
		let fixture = Fixture {
			invalid: kind == 0,
			fail_transport: kind == 1,
			fail_finish: kind == 2,
			fail_commit: kind == 3,
			budget: if kind == 4 { 2 } else { usize::MAX },
			..Fixture::new(Mode::Enforce)
		};
		let (result, context, original) =
			evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
		assert!(result.is_err(), "failure kind {kind}");
		assert_eq!(
			serde_json::to_value(context).unwrap(),
			serde_json::to_value(original).unwrap()
		);
		let saved = fixture.saved.lock().unwrap();
		assert_eq!(saved.applied, 0);
		assert_eq!(saved.permits.len(), if kind == 4 { 2 } else { 6 });
		assert!(
			!serde_json::to_string(&saved.evidence)
				.unwrap()
				.contains("RAW_PRIVATE_PROVIDER_BODY")
		);
	}
}

#[tokio::test]
async fn failed_finalization_preserves_every_dispatched_attempt_and_receipt() {
	for partial in [false, true] {
		let fixture = Fixture {
			fail_finish: !partial,
			fail_finish_result: partial,
			..Fixture::new(Mode::Enforce)
		};
		let (result, context, original) =
			evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
		assert!(result.is_err());
		assert_eq!(context.history, original.history);
		let saved = fixture.saved.lock().unwrap();
		let evidence = &saved.evidence[0];
		assert_eq!(evidence.outcome, Outcome::Rejected);
		assert_eq!(evidence.reason, Reason::JournalFailure);
		assert_eq!(evidence.summary().attempts, 6);
		assert_eq!(fixture.calls.load(Ordering::SeqCst), 6);
		assert_eq!(saved.answers, if partial { 3 } else { 0 });
		assert_eq!(evidence.answers.len(), saved.answers);
		for permit in &saved.permits {
			let attempt = evidence
				.attempts
				.iter()
				.find(|a| a.id == permit.record.attempt)
				.unwrap();
			assert_eq!(attempt.request_digest, permit.record.request_digest);
			assert_eq!(attempt.questions, permit.record.questions);
			assert_eq!(attempt.owner_receipts, permit.owner_receipts);
			let answered = partial && permit.record.questions[0].ends_with("_call");
			assert_eq!(
				attempt.status,
				if answered {
					AttemptStatus::Answered
				} else {
					AttemptStatus::Uncertain
				}
			);
		}
	}
}

#[tokio::test]
async fn contract_violations_and_transport_failures_have_distinct_rejection_reasons() {
	for kind in 0..3 {
		let fixture = Fixture {
			invalid: kind == 0,
			invalid_response: kind == 1,
			fail_transport: kind == 2,
			..Fixture::new(Mode::Enforce)
		};
		let (result, context, original) =
			evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
		assert!(result.is_err());
		assert_eq!(context.history, original.history);
		let saved = fixture.saved.lock().unwrap();
		let evidence = &saved.evidence[0];
		assert_eq!(
			evidence.reason,
			if kind == 2 {
				Reason::ProviderFailure
			} else {
				Reason::InvalidAnswers
			}
		);
		assert_eq!(evidence.summary().attempts, 6);
		assert!(evidence.answers.is_empty());
		assert!(evidence.attempts.iter().all(|a| a.status
			== if kind == 2 {
				AttemptStatus::Uncertain
			} else {
				AttemptStatus::Failed
			}));
	}
}

#[tokio::test]
async fn post_dispatch_revocation_commits_rejected_evidence_without_context_or_state() {
	for mode in [Mode::Enforce, Mode::Shadow] {
		let fixture = Fixture {
			revoke_after_dispatch: true,
			retain_state: true,
			..Fixture::new(mode)
		};
		let (result, context, original) = evaluate(&fixture, mode, 80_000, disclosure()).await;
		assert!(matches!(result, Err(Error::Forbidden)));
		assert_eq!(context.history, original.history);
		let saved = fixture.saved.lock().unwrap();
		assert_eq!(saved.applied, 0);
		assert_eq!(saved.states, 0);
		assert_eq!(saved.evidence.len(), 1);
		let evidence = &saved.evidence[0];
		assert_eq!(evidence.outcome, Outcome::Rejected);
		assert_eq!(evidence.reason, Reason::Forbidden);
		assert_eq!(evidence.summary().attempts, 6);
		assert_eq!(evidence.answers.len(), 6);
		assert!(
			evidence
				.attempts
				.iter()
				.all(|a| a.status == AttemptStatus::Answered)
		);
	}
}

#[tokio::test]
async fn invalid_final_approval_preserves_completed_attempts_without_context_or_state() {
	for mode in [Mode::Enforce, Mode::Shadow] {
		for (restrictions, days) in [
			(
				Restrictions {
					threshold: Probability::new(0.6).unwrap(),
					..Default::default()
				},
				7,
			),
			(
				Restrictions {
					preserve_recent: 5,
					..Default::default()
				},
				7,
			),
			(Restrictions::default(), 0),
			(Restrictions::default(), 31),
		] {
			let fixture = Fixture {
				retain_state: true,
				invalid_final_approval: Some(Approval {
					restrictions,
					state_retention: policy::StateRetention {
						enabled: true,
						days,
					},
				}),
				..Fixture::new(mode)
			};
			let (result, context, original) = evaluate(&fixture, mode, 80_000, disclosure()).await;
			assert!(result.is_err());
			assert_eq!(
				serde_json::to_value(context).unwrap(),
				serde_json::to_value(original).unwrap()
			);
			let saved = fixture.saved.lock().unwrap();
			assert_eq!(saved.applied, 0);
			assert_eq!(saved.states, 0);
			assert_eq!(saved.evidence.len(), 1);
			let evidence = &saved.evidence[0];
			assert_eq!(evidence.outcome, Outcome::Rejected);
			assert_eq!(evidence.reason, Reason::AuthorityFailure);
			assert_eq!(evidence.state, StateReference::Disabled);
			assert_eq!(fixture.calls.load(Ordering::SeqCst), 6);
			assert_eq!(saved.answers, 6);
			assert_eq!(evidence.answers.len(), 6);
			assert_eq!(evidence.attempts.len(), 6);
			for (attempt, permit) in evidence.attempts.iter().zip(&saved.permits) {
				assert_eq!(attempt.id, permit.record.attempt);
				assert_eq!(attempt.status, AttemptStatus::Answered);
				assert_eq!(attempt.request_digest, permit.record.request_digest);
				assert_eq!(attempt.questions, permit.record.questions);
				assert_eq!(attempt.owner_receipts, permit.owner_receipts);
			}
		}
	}
}

#[tokio::test]
async fn elapsed_state_deadlines_leave_only_an_expired_reference() {
	for mode in [Mode::Enforce, Mode::Shadow] {
		for during_commit in [false, true] {
			let now = Utc::now();
			let fixture = Fixture {
				retain_state: true,
				commit_time: (!during_commit).then_some(now + chrono::Duration::days(2)),
				expire_at_commit: during_commit,
				..Fixture::new(mode)
			};
			let mut view = disclosure();
			let deadline = now + chrono::Duration::days(1);
			view.source_expiry = Some(deadline);
			evaluate(&fixture, mode, 80_000, view).await.0.unwrap();
			let saved = fixture.saved.lock().unwrap();
			assert_eq!(saved.states, 0);
			let evidence = &saved.evidence[0];
			assert!(
				matches!(evidence.state, StateReference::Expired { expired_at, .. } if expired_at == deadline)
			);
			assert_eq!(evidence.replay().unwrap(), evidence.branches);
		}
	}
}

#[tokio::test]
async fn replay_rejects_impossible_reason_fit_and_mode_combinations() {
	let fixture = Fixture::new(Mode::Enforce);
	let fixture_shadow = Fixture::new(Mode::Shadow);
	for (fixture, mode, window) in [
		(&fixture, Mode::Enforce, 80_000),
		(&fixture, Mode::Enforce, 1),
		(&fixture_shadow, Mode::Shadow, 1),
	] {
		let _ = evaluate(fixture, mode, window, disclosure()).await;
		let evidence = fixture
			.saved
			.lock()
			.unwrap()
			.evidence
			.last()
			.unwrap()
			.clone();
		assert_eq!(evidence.replay().unwrap(), evidence.branches);
	}
	let applied = fixture.saved.lock().unwrap().evidence[0].clone();
	for reason in [
		Reason::Fits,
		Reason::Insufficient,
		Reason::Forbidden,
		Reason::ProviderFailure,
		Reason::AuthorityFailure,
		Reason::JournalFailure,
		Reason::InvalidAnswers,
		Reason::NoCandidates,
	] {
		let mut corrupt = applied.clone();
		corrupt.outcome = Outcome::Rejected;
		corrupt.reason = reason;
		assert!(corrupt.replay().is_err(), "rejected/{reason:?}");
	}
	let mut insufficient = applied.clone();
	insufficient.outcome = Outcome::Rejected;
	insufficient.reason = Reason::Insufficient;
	insufficient.fit.proposed = insufficient.fit.window + 1;
	// A decision without an effect names no projection.
	assert!(insufficient.replay().is_err());
	insufficient.projection_digest = None;
	assert_eq!(insufficient.replay().unwrap(), insufficient.branches);
	// The same pruning completed by a summary is staged and names its projection.
	let mut staged = insufficient.clone();
	staged.outcome = Outcome::Staged;
	assert!(staged.replay().is_err());
	staged.projection_digest = applied.projection_digest.clone();
	assert_eq!(staged.replay().unwrap(), staged.branches);
	let mut fitting = staged.clone();
	fitting.reason = Reason::Fits;
	fitting.fit.proposed = fitting.fit.window;
	assert!(
		fitting.replay().is_err(),
		"a fitting pruning is applied, not staged"
	);
	staged.mode = Mode::Shadow;
	assert!(staged.replay().is_err());
	insufficient.mode = Mode::Shadow;
	assert!(insufficient.replay().is_err());
	let mut forbidden = applied;
	forbidden.projection_digest = None;
	forbidden.outcome = Outcome::Rejected;
	forbidden.reason = Reason::Forbidden;
	forbidden.restrictions.forbid_apply = true;
	assert_eq!(forbidden.replay().unwrap(), forbidden.branches);
	forbidden.reason = Reason::Insufficient;
	assert!(forbidden.replay().is_err());
}

#[tokio::test]
async fn replay_rejects_change_metrics_that_disagree_with_historical_branches() {
	let fixture = Fixture::new(Mode::Enforce);
	evaluate(&fixture, Mode::Enforce, 80_000, disclosure())
		.await
		.0
		.unwrap();
	let evidence = fixture.saved.lock().unwrap().evidence[0].clone();
	assert_eq!(evidence.fit.dropped, 3);
	assert_eq!(evidence.replay().unwrap(), evidence.branches);
	for dropped in [evidence.fit.dropped - 1, evidence.fit.dropped + 1] {
		let mut corrupt = evidence.clone();
		corrupt.fit.dropped = dropped;
		corrupt.fit.retained = corrupt.history_length - dropped;
		assert_eq!(
			corrupt.fit.retained + corrupt.fit.dropped,
			corrupt.history_length
		);
		assert!(corrupt.replay().is_err());
	}
	let mut corrupt = evidence;
	corrupt.fit.truncated = 1;
	assert!(corrupt.replay().is_err());
}

#[rstest::rstest]
#[case(Mode::Enforce)]
#[case(Mode::Shadow)]
#[tokio::test]
async fn replay_rejects_models_that_differ_from_the_pinned_configuration(#[case] mode: Mode) {
	let fixture = Fixture::new(mode);
	evaluate(&fixture, mode, 80_000, disclosure())
		.await
		.0
		.unwrap();
	let evidence = fixture.saved.lock().unwrap().evidence[0].clone();
	let mut recovered: Evidence =
		serde_json::from_value(serde_json::to_value(evidence).unwrap()).unwrap();
	assert_eq!(recovered.replay().unwrap(), recovered.branches);
	recovered.model = "typesafe/jev-1.14".into();
	validate_model(&recovered.model).unwrap();
	assert!(
		recovered.replay().is_err(),
		"changed model in {mode:?} evidence"
	);
	recovered.model = config(mode).routes[0].model.clone();
	recovered.configuration_parameters_digest = format!("sha256:{}", "a".repeat(64));
	assert!(
		recovered.replay().is_err(),
		"changed configuration witness in {mode:?} evidence"
	);
}

#[tokio::test]
async fn replay_rejects_decider_pins_from_another_execution_node() {
	for mode in [Mode::Enforce, Mode::Shadow] {
		let fixture = Fixture::new(mode);
		evaluate(&fixture, mode, 80_000, disclosure())
			.await
			.0
			.unwrap();
		let evidence = fixture.saved.lock().unwrap().evidence[0].clone();
		let mut recovered: Evidence =
			serde_json::from_value(serde_json::to_value(evidence).unwrap()).unwrap();
		assert_eq!(recovered.replay().unwrap(), recovered.branches);
		recovered.decider.identity.registry_deployment = "aidash://foreign-decider".into();
		recovered.boundary.validate().unwrap();
		recovered.decider.validate().unwrap();
		assert!(
			recovered.replay().is_err(),
			"foreign Decider in {mode:?} evidence"
		);
	}
}

#[tokio::test]
async fn replay_rejects_unknown_or_overlapping_questions_for_every_attempt_status() {
	let fixture = Fixture::new(Mode::Enforce);
	evaluate(&fixture, Mode::Enforce, 80_000, disclosure())
		.await
		.0
		.unwrap();
	let evidence = fixture.saved.lock().unwrap().evidence[0].clone();
	for status in [
		AttemptStatus::Answered,
		AttemptStatus::Failed,
		AttemptStatus::Uncertain,
		AttemptStatus::NotDispatched,
	] {
		for questions in [
			vec!["unknown-question".to_owned()],
			evidence.attempts[0].questions.clone(),
			vec!["unknown-question".to_owned(); 2],
		] {
			let mut corrupt = evidence.clone();
			corrupt.attempts.push(AttemptEvidence {
				provider_report: Default::default(),
				id: Uuid::new_v4(),
				request_digest: format!("sha256:{}", "c".repeat(64)),
				question_digest: format!("sha256:{}", "d".repeat(64)),
				questions,
				status,
				owner_receipts: BTreeMap::from([("run-owner".into(), Uuid::new_v4())]),
			});
			assert!(corrupt.replay().is_err(), "fabricated {status:?} attempt");
		}
	}
}
/// #611: under `RULE` every kept candidate stays, so the request does not fit; the
/// opt-in `FIT_RULE` truncates kept results in ascending keep probability until it
/// fits, never one at the ceiling, and the evidence replays.
#[tokio::test]
async fn the_fit_rule_truncates_the_lowest_ranked_kept_results_until_the_request_fits() {
	let p = |value| Probability::new(value).unwrap();
	// Everything is kept by the 0.5 threshold; the calls stay as well.
	let mut answers = BTreeMap::new();
	for index in 0..10 {
		answers.insert(format!("q{index}_call"), p(0.6));
		answers.insert(format!("q{index}_result"), p(0.6));
	}
	answers.insert("q1_result".into(), p(0.55));
	answers.insert("q2_result".into(), p(0.52));
	answers.insert("q3_result".into(), p(aidash_domain::decision::FIT_CEILING));
	let keep = Fixture {
		answers: answers.clone(),
		..Fixture::new(Mode::Enforce)
	};
	let (result, _, _) = evaluate(&keep, Mode::Enforce, 85_000, disclosure()).await;
	let Ok(CompactionResult::Insufficient(staged)) = result else {
		panic!("the threshold rule keeps everything and cannot fit");
	};
	assert!(
		staged
			.evidence
			.branches
			.values()
			.all(|b| *b == Branch::Keep)
	);

	let fit = Fixture::fit(Mode::Enforce, answers);
	let (result, context, original) = evaluate(&fit, Mode::Enforce, 85_000, disclosure()).await;
	assert!(matches!(result.unwrap(), CompactionResult::Applied(_)));
	assert_eq!(context.history.len(), original.history.len());
	let saved = fit.saved.lock().unwrap();
	let evidence = &saved.evidence[0];
	assert_eq!(evidence.rule, aidash_domain::decision::FIT_RULE);
	assert_eq!(evidence.replay().unwrap(), evidence.branches);
	let branch_of = |index: usize| {
		let candidate = evidence
			.candidates
			.iter()
			.find(|c| c.history_index == index)
			.unwrap();
		evidence.branches[&candidate.id]
	};
	// The lowest-ranked result goes first, and a result at the ceiling is never touched.
	assert_eq!(branch_of(2), Branch::TruncateResult);
	assert_eq!(branch_of(3), Branch::Keep);
	assert!(evidence.fit.truncated >= 1);
	assert!(evidence.fit.proposed <= evidence.fit.window);
	// A recorded truncation out of rank order does not replay.
	let mut reordered = evidence.clone();
	let first = reordered
		.candidates
		.iter()
		.find(|c| c.history_index == 2)
		.unwrap()
		.id
		.clone();
	let third = reordered
		.candidates
		.iter()
		.find(|c| c.history_index == 3)
		.unwrap()
		.id
		.clone();
	reordered.branches.insert(first, Branch::Keep);
	reordered.branches.insert(third, Branch::TruncateResult);
	assert!(reordered.replay().is_err());
}
#[tokio::test]
async fn live_stricter_protection_replays_the_actual_preserved_branch() {
	let fixture = Fixture {
		stricter_after_dispatch: true,
		..Fixture::new(Mode::Enforce)
	};
	let (result, context, original) = evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
	// Pruning was not enough: nothing is recorded until a summary completes it.
	let Ok(CompactionResult::Insufficient(staged)) = result else {
		panic!("expected a staged decision");
	};
	assert_eq!(context.history, original.history);
	assert!(fixture.saved.lock().unwrap().evidence.is_empty());
	staged.rejected().validate_rejection().unwrap();
	let evidence = &staged.evidence;
	assert_eq!(evidence.reason, Reason::Insufficient);
	assert_eq!(evidence.restrictions.preserve_recent, 10);
	assert!(
		evidence
			.replay()
			.unwrap()
			.values()
			.all(|b| *b == Branch::Keep)
	);
}
#[tokio::test]
async fn forbidden_or_unknown_history_provenance_is_kept_and_never_questioned() {
	let fixture = Fixture::new(Mode::Enforce);
	let mut view = disclosure();
	view.tool_inputs.remove(&2);
	let (result, context, original) = evaluate(&fixture, Mode::Enforce, 90_000, view).await;
	result.unwrap();
	assert!(context.history.contains(&original.history[2]));
	assert!(
		fixture.saved.lock().unwrap().evidence[0]
			.candidates
			.iter()
			.all(|c| c.history_index != 2)
	);
	assert_eq!(fixture.calls.load(Ordering::SeqCst), 4);
}
#[tokio::test]
async fn bad_owner_receipts_and_revocation_never_allow_application() {
	for wrong in [false, true] {
		let fixture = Fixture {
			wrong_receipt: wrong,
			revoke_after_dispatch: !wrong,
			..Fixture::new(Mode::Enforce)
		};
		let (result, context, original) =
			evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
		assert!(result.is_err());
		assert_eq!(context.history, original.history);
		assert_eq!(fixture.saved.lock().unwrap().applied, 0);
		if wrong {
			assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
		}
	}
}
#[tokio::test]
async fn state_retention_is_prior_opt_in_and_replay_requires_current_reader_authority() {
	let fixture = Fixture {
		retain_state: true,
		reads_denied: true,
		..Fixture::new(Mode::Enforce)
	};
	evaluate(&fixture, Mode::Enforce, 80_000, disclosure())
		.await
		.0
		.unwrap();
	let evidence = fixture.saved.lock().unwrap().evidence[0].clone();
	assert_eq!(fixture.saved.lock().unwrap().states, 1);
	assert!(fixture.gate().replay(&evidence).await.is_err());
	let mut corrupt = evidence.clone();
	corrupt.version = 99;
	assert!(corrupt.replay().is_err());
	corrupt = evidence.clone();
	corrupt.rule = "compaction.keep/99".into();
	assert!(corrupt.replay().is_err());
	corrupt = evidence.clone();
	corrupt.branches.clear();
	assert!(corrupt.replay().is_err());
	corrupt = evidence.clone();
	corrupt.answers.remove(&corrupt.candidates[0].keep_call);
	assert!(corrupt.replay().is_err());
	corrupt = evidence;
	corrupt.attempts.clear();
	assert!(corrupt.replay().is_err());
}
#[test]
fn states_are_abridged_in_stages_and_beyond_the_long_context_budget_pause() {
	let mut context = history();
	context.history = (0..600).map(|_| context.history[0].clone()).collect();
	let boundary = Boundary {
		node: "aidash://execution".into(),
		run: Uuid::new_v4(),
		step: 1,
		run_revision: 1,
		input_revision: 1,
		worker: Uuid::new_v4(),
		input_digest: format!("sha256:{}", "a".repeat(64)),
		cycle: 0,
		retry: 0,
	};
	let inputs = |count: usize| -> BTreeMap<usize, Value> {
		(0..count)
			.map(|i| (i, json!({"query":"permitted".repeat(256)})))
			.collect()
	};
	let budget = StateBudget::default();
	let build = |view: &Disclosure| {
		build_compaction_state(
			&context,
			&boundary,
			view,
			&Restrictions::default(),
			&budget,
			false,
		)
	};
	// Five complete inputs fit the default mode as they are.
	let few = Disclosure {
		goal: Some("goal".into()),
		tool_inputs: inputs(5),
		..Default::default()
	};
	let built = build(&few).unwrap();
	assert!(serde_json::to_vec(&built.state).unwrap().len() <= budget.default_state_bytes);
	assert!(built.state.to_string().contains(&"permitted".repeat(256)));
	assert_eq!(built.candidates.len(), 4);
	// One large old input leaves the state below the default state budget,
	// but its question repeats it past the default request budget, so the
	// next stage abridges it.
	let large = Disclosure {
		goal: Some("goal".into()),
		tool_inputs: BTreeMap::from([
			(1, json!({"query":"x".repeat(17_000)})),
			(2, json!({"query":"recent"})),
		]),
		..Default::default()
	};
	let built = build(&large).unwrap();
	assert!(serde_json::to_vec(&built.state).unwrap().len() <= budget.default_state_bytes);
	assert!(!built.state.to_string().contains(&"x".repeat(17_000)));
	// Sixty inputs are abridged until the state fits the default mode.
	let more = Disclosure {
		goal: Some("goal".into()),
		tool_inputs: inputs(60),
		..Default::default()
	};
	let built = build(&more).unwrap();
	assert!(serde_json::to_vec(&built.state).unwrap().len() <= budget.default_state_bytes);
	assert!(!built.state.to_string().contains(&"permitted".repeat(256)));
	assert_eq!(built.candidates.len(), 59);
	// The most abridged state may still use the long-context mode.
	let many = Disclosure {
		goal: Some("g".repeat(100_000)),
		tool_inputs: inputs(600),
		..Default::default()
	};
	let built = build(&many).unwrap();
	let bytes = serde_json::to_vec(&built.state).unwrap().len();
	assert!(bytes > budget.default_state_bytes);
	assert_eq!(
		budget.request_limit(bytes),
		budget.long_context_request_bytes
	);
	assert!(built.fits);
	// Conversation text is never abridged, so a state beyond the long-context
	// budget comes back unfit: the Route refuses it and records why.
	let too_large = Disclosure {
		goal: Some("g".repeat(1_100_000)),
		tool_inputs: inputs(600),
		..Default::default()
	};
	assert!(!build(&too_large).unwrap().fits);
}

#[tokio::test]
async fn reservation_wait_cannot_outlive_authority_or_source_deadline() {
	for mode in [Mode::Enforce, Mode::Shadow] {
		for expired in [false, true] {
			let now = Utc::now();
			let deadline = now + chrono::Duration::hours(1);
			let fixture = Fixture {
				revoke_in_reserve: !expired,
				advance_on_reserve: expired.then_some(deadline),
				..Fixture::new(mode)
			};
			let mut view = disclosure();
			view.source_expiry = Some(deadline);
			let (result, context, original) = evaluate(&fixture, mode, 80_000, view).await;
			assert!(matches!(result, Err(Error::Forbidden)));
			assert_eq!(context.history, original.history);
			assert_eq!(
				fixture.calls.load(Ordering::SeqCst),
				0,
				"reservation disclosed expired/revoked state"
			);
			let saved = fixture.saved.lock().unwrap();
			assert!(!saved.permits.is_empty());
			assert_eq!(saved.evidence[0].attempts.len(), saved.permits.len());
			assert_eq!(saved.evidence[0].reason, Reason::Forbidden);
		}
	}
}

#[tokio::test]
async fn batch_setup_preserves_authority_and_journal_error_categories() {
	for kind in 0..3 {
		let mut fixture = Fixture {
			setup_error: Some(kind),
			recovery_run: Some(Uuid::new_v4()),
			..Fixture::new(Mode::Enforce)
		};
		let (result, context, original) =
			evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
		assert!(
			matches!(
				(kind, result),
				(0, Err(Error::Domain(aidash_domain::Error::Invalid(_))))
					| (1, Err(Error::Conflict(_)))
					| (2, Err(Error::External(_)))
			),
			"original setup error category {kind} was lost"
		);
		assert_eq!(context.history, original.history);
		assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
		let rejected = {
			let saved = fixture.saved.lock().unwrap();
			assert_ne!(saved.evidence[0].reason, Reason::ProviderFailure);
			saved.evidence[0].clone()
		};
		fixture.setup_error = None;
		fixture.run_revision += 1;
		let (result, context, _) = evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
		assert!(matches!(result, Ok(CompactionResult::Applied(_))));
		assert_eq!(context.history.len(), 7);
		let saved = fixture.saved.lock().unwrap();
		assert_eq!(saved.evidence.len(), 2);
		assert_eq!(saved.evidence[0], rejected);
		assert_eq!(saved.evidence[1].id, rejected.id);
		assert_eq!(saved.permits.len(), 6);
		assert_eq!(fixture.calls.load(Ordering::SeqCst), 6);
	}
}

#[tokio::test]
async fn new_worker_reuses_charged_attempts_after_uncommitted_decision() {
	for cut in 0..7 {
		let mut fixture = Fixture {
			recovery_run: Some(Uuid::new_v4()),
			fail_commit: true,
			fail_finish: cut == 1,
			fail_transport: cut == 2,
			fail_finish_result: cut == 3,
			..Fixture::new(Mode::Enforce)
		};
		let (result, context, original) =
			evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
		assert!(result.is_err());
		assert_eq!(context.history, original.history);
		assert_eq!(fixture.calls.load(Ordering::SeqCst), 6);
		let original_permits = fixture.saved.lock().unwrap().permits.clone();
		fixture.fail_commit = false;
		fixture.fail_finish = false;
		fixture.fail_transport = false;
		fixture.fail_finish_result = false;
		fixture.omit_recovery = cut == 4;
		fixture.fail_prepare = cut == 0;
		fixture.denied.store(cut == 5, Ordering::SeqCst);
		fixture.run_revision += 1;
		fixture.plan_revision = usize::from(cut == 6);
		let (result, context, original) =
			evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
		if cut == 0 || cut == 4 {
			assert!(matches!(result, Ok(CompactionResult::Applied(_))));
			assert_eq!(context.history.len(), 7);
		} else if cut == 5 {
			assert!(matches!(result, Err(Error::Forbidden)));
			assert_eq!(context.history, original.history);
		} else {
			// A changed plan conflicts; a charged attempt whose result is
			// unknown pauses the Run until a resume starts a new decision.
			assert!(
				matches!(
					result,
					Err(Error::Conflict(_)
						| Error::Context(
							aidash_domain::context::recovery::Failure::DeciderUnavailable
						))
				),
				"{cut}: {result:?}"
			);
			assert_eq!(context.history, original.history);
		}
		assert_eq!(
			fixture.calls.load(Ordering::SeqCst),
			6,
			"recovery redispatched charged requests"
		);
		let saved = fixture.saved.lock().unwrap();
		assert_eq!(saved.permits, original_permits);
		let evidence = saved.evidence.last().unwrap();
		assert_ne!(
			evidence.boundary.run_revision,
			original_permits[0].record.boundary.run_revision
		);
		assert_ne!(
			evidence.boundary.worker,
			original_permits[0].record.boundary.worker
		);
		assert_eq!(evidence.id, original_permits[0].record.decision);
		assert_eq!(evidence.attempts.len(), 6);
	}
}

#[tokio::test]
async fn replay_rejects_erased_retention_markers() {
	for expired in [false, true] {
		let fixture = Fixture {
			retain_state: true,
			expire_at_commit: expired,
			..Fixture::new(Mode::Enforce)
		};
		evaluate(&fixture, Mode::Enforce, 80_000, disclosure())
			.await
			.0
			.unwrap();
		let mut evidence = fixture.saved.lock().unwrap().evidence[0].clone();
		assert_eq!(evidence.replay().unwrap(), evidence.branches);
		let mut serialized = serde_json::to_value(&evidence).unwrap();
		serialized
			.as_object_mut()
			.unwrap()
			.remove("state_retention");
		assert!(serde_json::from_value::<Evidence>(serialized).is_err());
		let mut old = evidence.clone();
		old.version = 1;
		assert!(old.replay().is_err());
		evidence.state = StateReference::Disabled;
		assert!(
			evidence.replay().is_err(),
			"erased retention marker was accepted"
		);
	}
}

#[tokio::test]
async fn bound_decider_restrictions_reach_compaction_without_separate_defaults() {
	for restrictions in [
		Restrictions {
			threshold: Probability::new(0.1).unwrap(),
			..Default::default()
		},
		Restrictions {
			preserve_recent: 8,
			..Default::default()
		},
		Restrictions {
			forbid_apply: true,
			..Default::default()
		},
	] {
		let fixture = Fixture {
			binding_restrictions: restrictions.clone(),
			keep: Probability::new(0.25).unwrap(),
			..Fixture::new(Mode::Enforce)
		};
		let (result, context, original) =
			evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
		assert_eq!(context.history, original.history);
		let saved = fixture.saved.lock().unwrap();
		// A pruning that is not enough is handed back uncommitted.
		let staged = match &result {
			Ok(CompactionResult::Insufficient(staged)) => Some(staged.evidence.clone()),
			Ok(_) => panic!("the protected context cannot fit"),
			Err(_) => None,
		};
		let evidence = staged.as_ref().unwrap_or_else(|| &saved.evidence[0]);
		assert_eq!(evidence.restrictions, restrictions);
		if restrictions.forbid_apply {
			assert!(matches!(result, Err(Error::Forbidden)));
			assert!(saved.permits.is_empty());
		} else if restrictions.preserve_recent == 8 {
			assert_eq!(evidence.fit.dropped, 1);
			assert!(evidence.candidates.iter().all(|c| c.history_index < 2));
		} else {
			assert!(evidence.branches.values().all(|b| *b == Branch::Keep));
		}
	}
	let fixture = Fixture {
		corrupt_bound_config: true,
		..Fixture::new(Mode::Enforce)
	};
	let (result, context, original) = evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
	assert!(matches!(result, Err(Error::Invalid(_))));
	assert_eq!(context.history, original.history);
	assert_eq!(fixture.checks.load(Ordering::SeqCst), 0);
	assert!(fixture.saved.lock().unwrap().permits.is_empty());
}

/// Native Runs are pruned by Model Turn: the first and the latest six turns are
/// never classified, a turn whose calls are all dropped and that has no text
/// leaves whole, and events outside any turn stay (ADR 0015).
#[tokio::test]
async fn native_decisions_prune_whole_model_turns_and_keep_markers() {
	use aidash_domain::context::RequestProjection;
	use aidash_domain::projection::{CacheScope, native::model_turns};
	let fixture = Fixture::new(Mode::Enforce);
	let correction = ContextEvent::CitationCorrectionRequired {
		invalid: vec!["ev_unknown".into()],
		instruction: "Correct the answer once.".into(),
		through_seq: 1,
	};
	let ids = |turn: usize| [format!("t{turn}a"), format!("t{turn}b")];
	let mut context = Context::default();
	for turn in 0..12 {
		context.push(ContextEvent::Assistant {
			epoch: turn as i64,
			text: String::new(),
			calls: ids(turn).to_vec(),
		});
		for id in ids(turn) {
			context.push(ContextEvent::Tool {
				call: aidash_domain::provider::ToolCall {
					id: id.clone(),
					name: "safe_tool".into(),
					arguments: json!({"query":"permitted"}),
				},
				result: json!("r".repeat(4_000)),
			});
		}
		if turn == 2 {
			context.push(correction.clone());
		}
	}
	let pinned = json!({"task":{"description":"Ship the release"}});
	let projection = RequestProjection::Native(CacheScope {
		tenant: "tenant".into(),
		key_version: 1,
	});
	let full = RequestBudget {
		input_format: Default::default(),
		window: usize::MAX,
		instructions: "Work on the task",
		tools: &[],
		max_output_tokens: 0,
		projection: &projection,
		estimate: Default::default(),
	}
	.request(&context, &pinned)
	.estimated_total_tokens();
	let budget = RequestBudget {
		input_format: Default::default(),
		window: full * 3 / 4,
		instructions: "Work on the task",
		tools: &[],
		max_output_tokens: 0,
		projection: &projection,
		estimate: Default::default(),
	};
	let boundary = Boundary {
		node: "aidash://execution".into(),
		run: Uuid::new_v4(),
		step: 1,
		run_revision: 1,
		input_revision: 2,
		worker: Uuid::new_v4(),
		input_digest: compaction_input_digest(&context, &budget, &pinned).unwrap(),
		cycle: 0,
		retry: 0,
	};
	let definition = definition(Mode::Enforce);
	let decider = BoundDecider {
		pin: DeciderPin {
			identity: aidash_domain::registry::bindings::QualifiedRef {
				registry_deployment: boundary.node.clone(),
				id: definition.id.clone(),
				version: definition.version.clone(),
			},
			definition_digest: digest(&serde_json::to_value(&definition).unwrap()),
			configuration_digest: config(Mode::Enforce).digest().unwrap(),
			provider_implementation: "node-decision-adapter:23".into(),
		},
		config: config(Mode::Enforce),
		restrictions: Restrictions::default(),
	};
	let view = compaction_disclosure(&context, &pinned);
	let input = Evaluation {
		boundary: &boundary,
		decider: &decider,
		definition: &definition,
		disclosure: &view,
		now: Utc::now(),
	};
	// Act
	let result = fixture
		.gate()
		.compact(&mut context, &budget, &pinned, &input)
		.await;
	// Assert: the five unprotected turns left whole, the marker stayed.
	assert!(matches!(result.unwrap(), CompactionResult::Applied(_)));
	let turns: Vec<Vec<String>> = {
		let mut grouped: BTreeMap<usize, Vec<String>> = BTreeMap::new();
		for (entry, turn) in context.history.iter().zip(model_turns(&context.history)) {
			if let (Some(turn), ContextEvent::Tool { call, .. }) = (turn, &entry.event) {
				grouped.entry(turn).or_default().push(call.id.clone());
			}
		}
		grouped.into_values().collect()
	};
	assert_eq!(turns.len(), 7);
	assert_eq!(turns[0], ids(0).to_vec());
	assert_eq!(turns[1], ids(6).to_vec());
	assert_eq!(
		context
			.events()
			.filter(|event| **event == correction)
			.count(),
		1
	);
	let saved = fixture.saved.lock().unwrap();
	let evidence = saved.evidence.last().unwrap();
	assert_eq!(evidence.turns, Some(12));
	assert_eq!(evidence.candidates.len(), 10);
	assert!(evidence.candidates.iter().all(|c| c.turn.is_some()));
	assert_eq!(evidence.fit.dropped, 10);
	assert_eq!(evidence.fit.removed, 15);
	assert_eq!(evidence.replay().unwrap(), evidence.branches);
}

/// A resume cycle is part of the decision identity, so a person's resume starts
/// a new decision instead of meeting an attempt whose result is unknown.
#[test]
fn the_resume_cycle_changes_the_decision_identity() {
	let decider = DeciderPin {
		identity: aidash_domain::registry::bindings::QualifiedRef {
			registry_deployment: "aidash://execution".into(),
			id: "decider".into(),
			version: "1.0.0".into(),
		},
		definition_digest: format!("sha256:{}", "a".repeat(64)),
		configuration_digest: format!("sha256:{}", "b".repeat(64)),
		provider_implementation: "node-decision-adapter:23".into(),
	};
	let mut boundary = Boundary {
		node: "aidash://execution".into(),
		run: Uuid::from_u128(1),
		step: 1,
		run_revision: 1,
		input_revision: 2,
		worker: Uuid::from_u128(3),
		input_digest: format!("sha256:{}", "c".repeat(64)),
		cycle: 0,
		retry: 0,
	};
	let identity = |boundary: &Boundary| {
		decision_identity(
			boundary,
			&decider,
			&Restrictions::default(),
			&format!("sha256:{}", "d".repeat(64)),
			&[],
			1_000,
			0,
		)
	};
	let first = identity(&boundary);
	// A recovered lease keeps the decision.
	boundary.worker = Uuid::from_u128(4);
	boundary.run_revision = 2;
	assert_eq!(identity(&boundary), first);
	// Each outage retry is a decision of its own (ADR 0066).
	boundary.retry = 1;
	let retried = identity(&boundary);
	assert_ne!(retried, first);
	boundary.retry = 0;
	boundary.cycle = 1;
	assert_ne!(identity(&boundary), first);
	assert_ne!(identity(&boundary), retried);
	// So is each Route of one decision.
	boundary.cycle = 0;
	assert_ne!(
		decision_identity(
			&boundary,
			&decider,
			&Restrictions::default(),
			&format!("sha256:{}", "d".repeat(64)),
			&[],
			1_000,
			1,
		),
		first
	);
}

#[test]
fn a_question_budget_matches_the_largest_encoded_request_with_escaped_text() {
	let state = json!({"history": "quote\" and 日本語".repeat(1000)});
	let questions = Questions::from([
		(
			"q\"1".into(),
			aidash_domain::decision::Question {
				description: "short".into(),
				answer_type: AnswerType::Noul,
			},
		),
		(
			"q2".into(),
			aidash_domain::decision::Question {
				description: "a long \"quoted\" instruction".repeat(100),
				answer_type: AnswerType::Noul,
			},
		),
	]);
	let framing_reserve =
		super::state::single_question_request_bytes(&state, &Questions::new()).unwrap();
	let largest = questions.iter().map(|(id, question)| serde_json::to_vec(&json!({"state": state, "questions": { id: {"type": "noul", "instructions": question.description}}})).unwrap().len()).max().unwrap();
	assert_eq!(
		super::state::single_question_request_bytes(&state, &questions).unwrap(),
		largest + framing_reserve
	);
}

mod source_selection_tests;
