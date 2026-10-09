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
		provider_contract: PROVIDER.into(),
		endpoint: "https://example.test/systemone".into(),
		model: "jev-1.13.0".into(),
		credential_env: "AIDASH_SECRET_JEV".into(),
		builder: BUILDER.into(),
		option_source: OPTION_SOURCE.into(),
		rule: RULE.into(),
		keep_threshold: Probability::half(),
		mode,
	}
}
fn definition(mode: Mode) -> aidash_domain::registry::Entry {
	serde_json::from_value(json!({"id":"decider","version":"1.0.0","kind":"decider",
        "name":{"en":"Compaction"},"description":{"en":"Retain history"},"config":config(mode)}))
	.unwrap()
}
fn history() -> Context {
	Context {
		history: (0..10)
			.map(|index| ContextEvent::Tool {
				call: aidash_domain::provider::ToolCall {
					id: format!("call{index}"),
					name: "safe_tool".into(),
					arguments: json!({"private":"INPUT_SECRET","query":"permitted"}),
				},
				result: json!({"body":format!("RESULT_SECRET{}", "x".repeat(10_000))}),
			})
			.collect(),
		summary: "PRIVATE_SUMMARY".into(),
		..Default::default()
	}
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
	calls: AtomicUsize,
	checks: AtomicUsize,
	in_flight: AtomicUsize,
	peak: AtomicUsize,
	reads_denied: bool,
}
impl Fixture {
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
			calls: AtomicUsize::new(0),
			checks: AtomicUsize::new(0),
			in_flight: AtomicUsize::new(0),
			peak: AtomicUsize::new(0),
			reads_denied: false,
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
	async fn expire_states(&self, _: DateTime<Utc>) -> Result<usize> {
		Ok(0)
	}
	async fn append_outcome(&self, _: &OutcomeLink) -> Result<()> {
		Ok(())
	}
}
impl DecisionProvider for Fixture {
	fn implementation_id(&self) -> &str {
		&self.provider_implementation
	}
	fn configuration_digest(&self) -> Result<String> {
		Ok(self.configuration_digest.clone())
	}
	fn plan(&self, state: &Value, questions: &Questions) -> Result<Vec<PreparedRequest>> {
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
	fn prepare(&self, request: &PreparedRequest) -> Result<Box<dyn PreparedDispatch + '_>> {
		if self.fail_prepare {
			return Err(Error::Invalid("credential unavailable".into()));
		}
		self.preparations.fetch_add(1, Ordering::SeqCst);
		Ok(Box::new(FixtureDispatch {
			fixture: self,
			request: request.clone(),
		}))
	}
}
struct FixtureDispatch<'a> {
	fixture: &'a Fixture,
	request: PreparedRequest,
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
					if id.ends_with("_call") {
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
	context.history = (0..fixture.events)
		.map(|i| {
			let mut event = context.history[i % context.history.len()].clone();
			if let ContextEvent::Tool { call, .. } = &mut event {
				call.id = format!("call{i}");
			}
			event
		})
		.collect();
	let original = context.clone();
	let pinned = json!({"reference_documents":"DOCUMENT_SECRET"});
	let budget = RequestBudget {
		window,
		instructions: "SYSTEM_SECRET",
		tools: &[],
		max_output_tokens: 0,
		projection: Default::default(),
	};
	let mut boundary = Boundary {
		node: "aidash://execution".into(),
		run: fixture.recovery_run.unwrap_or_else(Uuid::new_v4),
		step: 1,
		run_revision: fixture.run_revision,
		input_revision: 2,
		worker: Uuid::new_v4(),
		input_digest: compaction_input_digest(&context, &budget, &pinned).unwrap(),
	};
	if fixture.corrupt_boundary {
		boundary.input_digest = format!("sha256:{}", "b".repeat(64));
	}
	let definition = definition(mode);
	let pin = DeciderPin {
		identity: aidash_domain::registry::bindings::QualifiedRef {
			registry_node: boundary.node.clone(),
			id: definition.id.clone(),
			version: definition.version.clone(),
		},
		definition_digest: digest(&serde_json::to_value(&definition).unwrap()),
		configuration_digest: config(mode).digest().unwrap(),
		provider_implementation: "node-decision-adapter:23".into(),
	};
	let mut bound = BoundDecider {
		pin,
		config: config(mode),
		restrictions: fixture.binding_restrictions.clone(),
	};
	if fixture.corrupt_bound_config {
		bound.config.model = "jev-1.14.0".into();
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
	assert_eq!(result.unwrap(), CompactionResult::AlreadyFits);
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
#[tokio::test]
async fn history_without_disclosure_proof_stays_protected_with_zero_attempts() {
	let fixture = Fixture::new(Mode::Enforce);
	let (result, context, original) =
		evaluate(&fixture, Mode::Enforce, 80_000, Disclosure::default()).await;
	assert!(result.is_err());
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
	let ContextEvent::Tool { call: before, .. } = &original.history[1] else {
		panic!()
	};
	let ContextEvent::Tool {
		call: after,
		result,
	} = &context.history[1]
	else {
		panic!()
	};
	assert_eq!(before, after);
	assert!(
		result
			.as_str()
			.unwrap()
			.contains("full result remains in the execution journal")
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
					keep_threshold: Probability::new(0.6).unwrap(),
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
	assert_eq!(insufficient.replay().unwrap(), insufficient.branches);
	insufficient.mode = Mode::Shadow;
	assert!(insufficient.replay().is_err());
	let mut forbidden = applied;
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
	recovered.model = "jev-1.14.0".into();
	validate_model(&recovered.model).unwrap();
	assert!(
		recovered.replay().is_err(),
		"changed model in {mode:?} evidence"
	);
	recovered.model = config(mode).model;
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
		recovered.decider.identity.registry_node = "aidash://foreign-decider".into();
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
				id: Uuid::new_v4(),
				request_digest: format!("sha256:{}", "c".repeat(64)),
				questions,
				status,
				owner_receipts: BTreeMap::from([("run-owner".into(), Uuid::new_v4())]),
			});
			assert!(corrupt.replay().is_err(), "fabricated {status:?} attempt");
		}
	}
}
#[tokio::test]
async fn live_stricter_protection_replays_the_actual_preserved_branch() {
	let fixture = Fixture {
		stricter_after_dispatch: true,
		..Fixture::new(Mode::Enforce)
	};
	let (result, context, original) = evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
	assert!(result.is_err());
	assert_eq!(context.history, original.history);
	let saved = fixture.saved.lock().unwrap();
	let evidence = &saved.evidence[0];
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
fn state_and_question_sizes_have_no_aidash_ceiling() {
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
	};
	let view = Disclosure {
		goal: Some("g".repeat(1_100_000)),
		tool_inputs: (0..600)
			.map(|i| (i, json!({"query":"permitted".repeat(256)})))
			.collect(),
		..Default::default()
	};
	let built =
		build_compaction_state(&context, &boundary, &view, &Restrictions::default()).unwrap();
	assert!(built.state.to_string().len() > 1_048_576);
	assert!(built.questions.len() > 1024);
	assert!(serde_json::to_vec(&built.questions).unwrap().len() > 1_048_576);
	assert!(serde_json::to_vec(&built.candidates).unwrap().len() > 1_048_576);
	assert_eq!(built.candidates.len(), 593);
	assert_eq!(built.state["goal"].as_str().unwrap().len(), 1_100_000);
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
			assert!(matches!(result, Err(Error::Conflict(_))));
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
			keep_threshold: Probability::new(0.1).unwrap(),
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
		assert!(result.is_err());
		assert_eq!(context.history, original.history);
		let saved = fixture.saved.lock().unwrap();
		let evidence = &saved.evidence[0];
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
