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
	answers: usize,
	evidence: Vec<Evidence>,
	applied: usize,
	states: usize,
}
struct Fixture {
	events: usize,
	corrupt_boundary: bool,
	configuration_digest: String,
	saved: Mutex<Saved>,
	budget: usize,
	invalid: bool,
	fail_transport: bool,
	fail_finish: bool,
	fail_commit: bool,
	wrong_receipt: bool,
	revoke_after_dispatch: bool,
	denied: AtomicBool,
	retain_state: bool,
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
			configuration_digest: config(mode).digest().unwrap(),
			saved: Mutex::new(Saved {
				permits: vec![],
				answers: 0,
				evidence: vec![],
				applied: 0,
				states: 0,
			}),
			budget: usize::MAX,
			invalid: false,
			fail_transport: false,
			fail_finish: false,
			fail_commit: false,
			wrong_receipt: false,
			revoke_after_dispatch: false,
			denied: AtomicBool::new(false),
			retain_state: false,
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
		self.checks.fetch_add(1, Ordering::SeqCst);
		if self.denied.load(Ordering::SeqCst) {
			return Err(Error::Forbidden);
		}
		Ok(Approval {
			restrictions: Restrictions {
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
	async fn reserve(&self, record: &DispatchRecord) -> Result<DispatchPermit> {
		let mut saved = self.saved.lock().unwrap();
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
		Ok(permit)
	}
	async fn finish_attempt(
		&self,
		permit: &DispatchPermit,
		answers: Option<&BTreeMap<String, Probability>>,
		status: AttemptStatus,
	) -> Result<()> {
		assert!(self.saved.lock().unwrap().permits.contains(permit));
		if self.fail_finish {
			return Err(Error::External("journal failed".into()));
		}
		assert_eq!(answers.is_some(), status == AttemptStatus::Answered);
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
		assert_eq!(
			state.is_some(),
			matches!(evidence.state, StateReference::Retained { .. })
		);
		if let Some(state) = state {
			assert_eq!(digest(&state.state), evidence.state_digest);
		}
		let mut saved = self.saved.lock().unwrap();
		saved.applied += usize::from(context.is_some());
		saved.states += usize::from(state.is_some());
		saved.evidence.push(evidence.clone());
		Ok(())
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
				body: serde_json::to_vec(&json!({"state":state,"question":q})).unwrap(),
				questions: BTreeMap::from([(id.clone(), q.clone())]),
			})
			.collect())
	}
	fn preflight(&self, _: &PreparedRequest) -> Result<()> {
		Ok(())
	}
	async fn dispatch(&self, request: &PreparedRequest) -> Result<BTreeMap<String, Probability>> {
		// Assert reservation-before-I/O and exact physical request digest.
		assert!(
			self.saved
				.lock()
				.unwrap()
				.permits
				.iter()
				.any(|p| p.record.request_digest == request.digest())
		);
		self.calls.fetch_add(1, Ordering::SeqCst);
		let concurrent = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
		self.peak.fetch_max(concurrent, Ordering::SeqCst);
		tokio::task::yield_now().await;
		self.in_flight.fetch_sub(1, Ordering::SeqCst);
		if self.revoke_after_dispatch {
			self.denied.store(true, Ordering::SeqCst);
		}
		if self.fail_transport {
			return Err(Error::External("RAW_PRIVATE_PROVIDER_BODY".into()));
		}
		if self.invalid {
			return Ok(BTreeMap::new());
		}
		Ok(request
			.questions
			.keys()
			.map(|id| {
				(
					id.clone(),
					if id.ends_with("_call") {
						self.keep_call.unwrap_or(self.keep)
					} else {
						self.keep
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
	};
	let mut boundary = Boundary {
		node: "aidash://execution".into(),
		run: Uuid::new_v4(),
		step: 1,
		run_revision: 1,
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
	};
	let restrictions = Restrictions::default();
	let input = Evaluation {
		boundary: &boundary,
		decider: &pin,
		definition: &definition,
		restrictions: &restrictions,
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
	for corrupt in [false, true] {
		let mut fixture = Fixture {
			corrupt_boundary: corrupt,
			..Fixture::new(Mode::Enforce)
		};
		if !corrupt {
			fixture.configuration_digest = format!("sha256:{}", "c".repeat(64));
		}
		let (result, context, original) =
			evaluate(&fixture, Mode::Enforce, 80_000, disclosure()).await;
		assert!(result.is_err());
		assert_eq!(context.history, original.history);
		assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
		assert!(fixture.saved.lock().unwrap().permits.is_empty());
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
