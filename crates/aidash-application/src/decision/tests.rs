#![allow(dead_code)]
use super::*;
use async_trait::async_trait;
use std::sync::{
	Mutex,
	atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn config(mode: Mode) -> DeciderConfig {
	DeciderConfig {
		hook: Hook::ContextSelection,
		answer_type: AnswerType::Noul,
		description: "Keep needed history".into(),
		provider_credential: "openrouter".into(),
		routes: vec![DeciderRoute::jev("typesafe/jev-1.13")],
		builder: SOURCE_BUILDER.into(),
		option_source: SOURCE_OPTION.into(),
		rule: SOURCE_RULE.into(),
		mode,
		max_calls_per_run: 1_000,
		state_retention: policy::StateRetention::default(),
	}
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

struct Saved {
	permits: Vec<DispatchPermit>,
	completed: BTreeMap<Uuid, (AttemptStatus, Option<BTreeMap<String, Probability>>)>,
	answers: usize,
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
			rule: SOURCE_RULE,
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

	async fn commit_approval(&self, _: &ApprovalEvidence) -> Result<()> {
		unreachable!("source selection commits no approval evidence")
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

mod source_selection_tests;

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
