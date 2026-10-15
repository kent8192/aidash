//! Decision Evidence and Decision Attempts persist behind the portable gate:
//! charged attempts are recovered, never charged twice, and an applied decision
//! commits its context and evidence atomically under the Run's lease.
#[path = "support/legacy.rs"]
mod common;

use aidash_application::{
	decision::{CompactionResult, DecisionGate, Evaluation, compaction_input_digest},
	ports::decision::{
		Approval, DecisionAuthority, DecisionJournal, DecisionProvider, Disclosure, DispatchError,
		DispatchRecord, PreparedDispatch, PreparedRequest, Reservation,
	},
};
use aidash_domain::{
	context::{Context, ContextEvent, RequestBudget, RequestProjection, recovery::Failure},
	decision::{
		policy::{ExecutionAllowance, StateRetention},
		*,
	},
	provider::ToolCall,
	registry::{Entry, bindings::QualifiedRef, rules::digest},
};
use aidash_server::{
	apps::execution::repositories::decisions::{CurrentAllowance, RunDecisionJournal},
	database::native,
	domain::Run,
	federation::Federation,
	store::Store,
};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use common::{
	ApplicationFixture, TestApplication, bootstrap, cleanup, native_application, request,
};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{
	Mutex,
	atomic::{AtomicBool, AtomicUsize, Ordering},
};
use uuid::Uuid;

const IMPLEMENTATION: &str = "node-decision-adapter:fixture";
const WINDOW: usize = 80_000;

/// The only place that spells a Decider configuration, so a change of its
/// shape touches one helper.
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

fn definition(mode: Mode) -> Entry {
	serde_json::from_value(
		json!({"id":"compaction-decider","version":"1.0.0","kind":"decider",
		"name":{"en":"Compaction"},"description":{"en":"Retain history"},"config":config(mode)}),
	)
	.unwrap()
}

fn bound(node: &str, mode: Mode) -> BoundDecider {
	let definition = definition(mode);
	BoundDecider {
		pin: DeciderPin {
			identity: QualifiedRef {
				registry_node: node.into(),
				id: definition.id.clone(),
				version: definition.version.clone(),
			},
			definition_digest: digest(&serde_json::to_value(&definition).unwrap()),
			configuration_digest: config(mode).digest().unwrap(),
			provider_implementation: IMPLEMENTATION.into(),
		},
		config: config(mode),
		restrictions: Restrictions::default(),
	}
}

fn allowance(decider: &BoundDecider, max_calls_per_run: u64) -> ExecutionAllowance {
	ExecutionAllowance {
		decider: decider.pin.clone(),
		max_calls_per_run,
	}
}

/// The Run's current decision policy; a test lowers, revokes or repins it
/// between reservations.
struct Policy(Mutex<Option<ExecutionAllowance>>);
impl Policy {
	fn new(allowance: ExecutionAllowance) -> Self {
		Self(Mutex::new(Some(allowance)))
	}
	fn set(&self, allowance: Option<ExecutionAllowance>) {
		*self.0.lock().unwrap() = allowance;
	}
}
#[async_trait]
impl CurrentAllowance for Policy {
	async fn current(
		&self,
		_: &mut native::Transaction,
		_: &Run,
	) -> aidash_server::Result<Option<ExecutionAllowance>> {
		Ok(self.0.lock().unwrap().clone())
	}
}

/// Answers every question in one request with the same keep probability.
struct Provider {
	digest: String,
	keep: Probability,
	calls: AtomicUsize,
}
impl Provider {
	fn new(mode: Mode) -> Self {
		Self {
			digest: config(mode).digest().unwrap(),
			keep: Probability::new(0.0).unwrap(),
			calls: AtomicUsize::new(0),
		}
	}
}
impl DecisionProvider for Provider {
	fn implementation_id(&self) -> &str {
		IMPLEMENTATION
	}
	fn configuration_digest(&self) -> aidash_application::Result<String> {
		Ok(self.digest.clone())
	}
	fn plan(
		&self,
		state: &Value,
		questions: &Questions,
	) -> aidash_application::Result<Vec<PreparedRequest>> {
		Ok(vec![PreparedRequest {
			body: serde_json::to_vec(&json!({"state":state,"questions":questions})).unwrap(),
			questions: questions.clone(),
		}])
	}
	fn prepare(
		&self,
		request: &PreparedRequest,
	) -> aidash_application::Result<Box<dyn PreparedDispatch + '_>> {
		Ok(Box::new(Dispatch {
			provider: self,
			request: request.clone(),
		}))
	}
}
struct Dispatch<'a> {
	provider: &'a Provider,
	request: PreparedRequest,
}
#[async_trait]
impl PreparedDispatch for Dispatch<'_> {
	async fn dispatch(self: Box<Self>) -> Result<BTreeMap<String, Probability>, DispatchError> {
		self.provider.calls.fetch_add(1, Ordering::SeqCst);
		Ok(self
			.request
			.questions
			.keys()
			.map(|id| (id.clone(), self.provider.keep))
			.collect())
	}
}

/// Approves every check; `forbid_after_dispatch` forbids applying once the
/// provider has answered, and `retain_state` keeps the classification state.
struct Authority<'a> {
	provider: &'a Provider,
	forbid_after_dispatch: AtomicBool,
	retain_state: bool,
}
#[async_trait]
impl DecisionAuthority for Authority<'_> {
	async fn check(
		&self,
		_: &Boundary,
		_: &DeciderPin,
		_: &[SourcePin],
	) -> aidash_application::Result<Approval> {
		let forbid = self.forbid_after_dispatch.load(Ordering::SeqCst)
			&& self.provider.calls.load(Ordering::SeqCst) > 0;
		Ok(Approval {
			restrictions: Restrictions {
				forbid_apply: forbid,
				..Default::default()
			},
			state_retention: StateRetention {
				enabled: self.retain_state,
				days: 7,
			},
		})
	}
	async fn read(&self, _: &Evidence, _: bool) -> aidash_application::Result<()> {
		Ok(())
	}
}

/// A leased Run whose journaled context holds ten large tool results.
async fn leased(f: &Federation, app: &TestApplication, token: &str) -> (Run, Uuid) {
	let (status, created) = request(app, token, "POST", "/api/conversations", json!({
		"title":"Decisions", "goal":"Reply", "target":{"id":"research","version":"1.0.0"},"target_kind":"agent"
	}))
	.await;
	assert_eq!(status, 200, "{created}");
	let worker = Uuid::new_v4();
	let mut run = f
		.store
		.lease_run(worker, 60)
		.await
		.unwrap()
		.expect("leased Run");
	for index in 0..10 {
		run.context.push(ContextEvent::Tool {
			call: ToolCall {
				id: format!("call{index}"),
				name: "safe_tool".into(),
				arguments: json!({"query":"permitted"}),
			},
			result: json!({"body":"x".repeat(10_000)}),
		});
	}
	let run = f
		.store
		.save_run(&run, worker, "run.sources_observed")
		.await
		.unwrap();
	(run, worker)
}

fn budget(projection: &RequestProjection) -> RequestBudget<'_> {
	RequestBudget {
		window: WINDOW,
		instructions: "Follow the task",
		tools: &[],
		max_output_tokens: 0,
		projection,
		estimate: Default::default(),
	}
}

fn boundary(store: &Store, run: &Run, worker: Uuid, context: &Context) -> Boundary {
	let projection = RequestProjection::Legacy;
	Boundary {
		node: store.node_id.clone(),
		run: run.id,
		step: run.step,
		run_revision: run.revision,
		input_revision: run.observed_input_seq,
		worker,
		input_digest: compaction_input_digest(context, &budget(&projection), &json!({})).unwrap(),
	}
}

fn disclosure(context: &Context) -> Disclosure {
	Disclosure {
		goal: Some("Reply".into()),
		tool_inputs: context
			.events()
			.enumerate()
			.filter(|(_, event)| matches!(event, ContextEvent::Tool { .. }))
			.map(|(index, _)| (index, json!({"query":"permitted"})))
			.collect(),
		sources: vec![SourcePin {
			resource: "authorized-history".into(),
			revision_digest: format!("sha256:{}", "a".repeat(64)),
		}],
		..Default::default()
	}
}

/// Runs the real gate against the PostgreSQL journal of `run`.
async fn decide(
	store: &Store,
	run: &Run,
	worker: Uuid,
	mode: Mode,
	authority: &Authority<'_>,
	provider: &Provider,
) -> (aidash_application::Result<CompactionResult>, Context) {
	let decider = bound(&store.node_id, mode);
	let policy = Policy::new(allowance(&decider, 10));
	let journal =
		RunDecisionJournal::new(store, run.clone(), allowance(&decider, 10), &policy).unwrap();
	let mut context = run.context.clone();
	let boundary = boundary(store, run, worker, &context);
	let definition = definition(mode);
	let view = disclosure(&context);
	let projection = RequestProjection::Legacy;
	let gate = DecisionGate {
		authority,
		journal: &journal,
		provider,
	};
	let input = Evaluation {
		boundary: &boundary,
		decider: &decider,
		definition: &definition,
		disclosure: &view,
		now: Utc::now(),
	};
	let result = gate
		.compact(&mut context, &budget(&projection), &json!({}), &input)
		.await;
	(result, context)
}

async fn evidence_rows(store: &Store, run: Uuid) -> Vec<(String, Evidence)> {
	let rows = sqlx::query(
		&Query::select()
			.columns(["outcome", "evidence"].map(Alias::new))
			.from(Alias::new("decision_evidence"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.order_by(Alias::new("created_at"), Order::Asc)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(store.pool.driver())
	.await
	.unwrap();
	use sqlx::Row as _;
	rows.into_iter()
		.map(|row| {
			(
				row.get("outcome"),
				serde_json::from_value(row.get::<Value, _>("evidence")).unwrap(),
			)
		})
		.collect()
}

async fn count(store: &Store, table: &str, run: Uuid) -> i64 {
	use sqlx::Row as _;
	sqlx::query(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new(table))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(store.pool.driver())
	.await
	.unwrap()
	.get(0)
}

fn record(
	store: &Store,
	run: &Run,
	worker: Uuid,
	decider: &BoundDecider,
	questions: &[&str],
	request: char,
) -> DispatchRecord {
	DispatchRecord {
		attempt: Uuid::new_v4(),
		decision: Uuid::nil(),
		boundary: Boundary {
			node: store.node_id.clone(),
			run: run.id,
			step: run.step,
			run_revision: run.revision,
			input_revision: run.observed_input_seq,
			worker,
			input_digest: format!("sha256:{}", "c".repeat(64)),
		},
		decider: decider.pin.clone(),
		binding_restrictions: Restrictions::default(),
		request_digest: format!("sha256:{}", request.to_string().repeat(64)),
		state_digest: format!("sha256:{}", "d".repeat(64)),
		questions: questions.iter().map(|q| (*q).to_owned()).collect(),
		mode: Mode::Enforce,
	}
}

#[rstest::rstest]
#[tokio::test]
async fn an_applied_decision_commits_its_context_and_evidence_atomically(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (run, worker) = leased(&f, &app, &token).await;
	let provider = Provider::new(Mode::Enforce);
	let authority = Authority {
		provider: &provider,
		forbid_after_dispatch: AtomicBool::new(false),
		retain_state: false,
	};
	// Act
	let (result, context) =
		decide(&f.store, &run, worker, Mode::Enforce, &authority, &provider).await;
	// Assert: the decision applied, and the saved Run holds the same context.
	assert!(matches!(result.unwrap(), CompactionResult::Applied(_)));
	assert!(context.history.len() < run.context.history.len());
	let saved = f.store.run(run.id).await.unwrap();
	assert_eq!(
		serde_json::to_value(&saved.context.history).unwrap(),
		serde_json::to_value(&context.history).unwrap()
	);
	assert_eq!(saved.revision, run.revision + 1);
	assert_eq!(
		saved.lease_owner, None,
		"an adopted compaction ends its step"
	);
	let rows = evidence_rows(&f.store, run.id).await;
	assert_eq!(rows.len(), 1);
	let (outcome, evidence) = &rows[0];
	assert_eq!(outcome, "applied");
	assert_eq!(evidence.replay().unwrap(), evidence.branches);
	assert_eq!(evidence.state, StateReference::Disabled);
	assert_eq!(count(&f.store, "decision_attempts", run.id).await, 1);
	assert_eq!(count(&f.store, "decision_states", run.id).await, 0);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_rejected_decision_resumes_with_its_charged_answers_and_ends_final(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (run, worker) = leased(&f, &app, &token).await;
	let provider = Provider::new(Mode::Enforce);
	let authority = Authority {
		provider: &provider,
		forbid_after_dispatch: AtomicBool::new(true),
		retain_state: false,
	};
	// Arrange: the final approval forbids applying after the provider answered.
	let (rejected, unchanged) =
		decide(&f.store, &run, worker, Mode::Enforce, &authority, &provider).await;
	assert!(rejected.is_err());
	assert_eq!(unchanged.history.len(), run.context.history.len());
	assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
	// Evidence whose answers differ from the charged result is never committed.
	let decider = bound(&f.store.node_id, Mode::Enforce);
	let policy = Policy::new(allowance(&decider, 10));
	let journal =
		RunDecisionJournal::new(&f.store, run.clone(), allowance(&decider, 10), &policy).unwrap();
	let (_, mut tampered) = evidence_rows(&f.store, run.id).await.remove(0);
	assert!(
		tampered
			.attempts
			.iter()
			.all(|attempt| attempt.status == AttemptStatus::Answered)
	);
	*tampered.answers.values_mut().next().unwrap() = Probability::new(1.0).unwrap();
	assert!(matches!(
		journal.commit(&tampered, None, None).await,
		Err(aidash_application::Error::Invalid(message)) if message.contains("charged attempts")
	));
	assert_eq!(evidence_rows(&f.store, run.id).await.len(), 1);
	// Act: the same decision resumes once applying is permitted again.
	authority
		.forbid_after_dispatch
		.store(false, Ordering::SeqCst);
	let (applied, _) = decide(&f.store, &run, worker, Mode::Enforce, &authority, &provider).await;
	// Assert: the charged answers were reused without another provider call,
	// and the rejected snapshot was kept beside the final one.
	assert!(matches!(applied.unwrap(), CompactionResult::Applied(_)));
	assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
	let rows = evidence_rows(&f.store, run.id).await;
	assert_eq!(
		rows.iter()
			.map(|(outcome, _)| outcome.as_str())
			.collect::<Vec<_>>(),
		vec!["rejected", "applied"]
	);
	assert_eq!(rows[0].1.reason, Reason::Forbidden);
	assert_eq!(rows[0].1.id, rows[1].1.id);
	assert_eq!(count(&f.store, "decision_attempts", run.id).await, 1);
	// An applied decision is final: recovery under a new lease reports a conflict.
	let worker = Uuid::new_v4();
	let run = f
		.store
		.lease_run(worker, 60)
		.await
		.unwrap()
		.expect("leased Run");
	let journal =
		RunDecisionJournal::new(&f.store, run.clone(), allowance(&decider, 10), &policy).unwrap();
	let boundary = boundary(&f.store, &run, worker, &run.context);
	assert!(matches!(
		journal.recover(rows[1].1.id, &boundary).await,
		Err(aidash_application::Error::Conflict(_))
	));
	f.store.release_lease(run.id, worker).await.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn retained_state_expires_to_its_marker(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (run, worker) = leased(&f, &app, &token).await;
	let provider = Provider::new(Mode::Shadow);
	let authority = Authority {
		provider: &provider,
		forbid_after_dispatch: AtomicBool::new(false),
		retain_state: true,
	};
	// Arrange: a Shadow decision keeps its state and leaves the Run unchanged.
	let (result, _) = decide(&f.store, &run, worker, Mode::Shadow, &authority, &provider).await;
	assert!(matches!(result.unwrap(), CompactionResult::Shadow(_)));
	assert_eq!(f.store.run(run.id).await.unwrap().revision, run.revision);
	assert_eq!(count(&f.store, "decision_states", run.id).await, 1);
	let (_, retained) = evidence_rows(&f.store, run.id).await.remove(0);
	assert!(matches!(retained.state, StateReference::Retained { .. }));
	// Act: nothing is due yet, then the deadline passes.
	assert_eq!(f.store.expire_decision_states(Utc::now()).await.unwrap(), 0);
	let expired = f
		.store
		.expire_decision_states(Utc::now() + Duration::days(8))
		.await
		.unwrap();
	// Assert: the bytes are gone, the marker matches the witness, replay still holds.
	assert_eq!(expired, 1);
	assert_eq!(count(&f.store, "decision_states", run.id).await, 0);
	let (outcome, evidence) = evidence_rows(&f.store, run.id).await.remove(0);
	assert_eq!(outcome, "shadow");
	let (
		StateReference::Retained {
			id,
			digest,
			expires_at,
		},
		StateReference::Expired {
			id: expired_id,
			digest: expired_digest,
			expired_at,
		},
	) = (&retained.state, &evidence.state)
	else {
		panic!("expected a retained state replaced by its expired marker");
	};
	assert_eq!(
		(id, digest, expires_at),
		(expired_id, expired_digest, expired_at)
	);
	assert_eq!(evidence.state_retention, retained.state_retention);
	assert_eq!(evidence.replay().unwrap(), evidence.branches);
	f.store.release_lease(run.id, worker).await.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_charged_attempt_is_recovered_and_never_charged_twice(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (run, worker) = leased(&f, &app, &token).await;
	let decider = bound(&f.store.node_id, Mode::Enforce);
	let policy = Policy::new(allowance(&decider, 2));
	let journal =
		RunDecisionJournal::new(&f.store, run.clone(), allowance(&decider, 2), &policy).unwrap();
	let mut first = record(
		&f.store,
		&run,
		worker,
		&decider,
		&["q1_call", "q1_result"],
		'e',
	);
	first.decision = Uuid::new_v4();
	// Act: reserve, then reserve the same attempt again.
	let Reservation::Fresh(permit) = journal.reserve(&first).await.unwrap() else {
		panic!("a new attempt is charged");
	};
	assert_eq!(permit.owner_receipts.len(), 1);
	let Reservation::Recovered(pending) = journal.reserve(&first).await.unwrap() else {
		panic!("a charged attempt is recovered");
	};
	// Assert: an unfinished reservation recovers as uncertain with the same receipts.
	assert_eq!(pending.status, AttemptStatus::Uncertain);
	assert_eq!(pending.permit, permit);
	let mut changed = first.clone();
	changed.request_digest = format!("sha256:{}", "f".repeat(64));
	assert!(matches!(
		journal.reserve(&changed).await,
		Err(aidash_application::Error::Conflict(_))
	));
	let mut overlapping = record(
		&f.store,
		&run,
		worker,
		&decider,
		&["q1_result", "q2_call"],
		'b',
	);
	overlapping.decision = first.decision;
	assert!(matches!(
		journal.reserve(&overlapping).await,
		Err(aidash_application::Error::Conflict(_))
	));
	let answers: BTreeMap<_, _> = first
		.questions
		.iter()
		.map(|q| (q.clone(), Probability::new(0.25).unwrap()))
		.collect();
	// A mixed-up permit never records a result for the charged attempt.
	let mut mixed = permit.clone();
	for receipt in mixed.owner_receipts.values_mut() {
		*receipt = Uuid::new_v4();
	}
	assert!(matches!(
		journal
			.finish_attempt(&mixed, Some(&answers), AttemptStatus::Answered)
			.await,
		Err(aidash_application::Error::Conflict(_))
	));
	let mut narrowed = permit.clone();
	narrowed.record.questions.truncate(1);
	let narrowed_answers =
		BTreeMap::from([(first.questions[0].clone(), Probability::new(0.25).unwrap())]);
	assert!(matches!(
		journal
			.finish_attempt(&narrowed, Some(&narrowed_answers), AttemptStatus::Answered)
			.await,
		Err(aidash_application::Error::Conflict(_))
	));
	let Reservation::Recovered(still) = journal.reserve(&first).await.unwrap() else {
		panic!("a charged attempt is recovered");
	};
	assert_eq!(still.status, AttemptStatus::Uncertain);
	// Answers become the only reusable recovery source.
	journal
		.finish_attempt(&permit, Some(&answers), AttemptStatus::Answered)
		.await
		.unwrap();
	journal
		.finish_attempt(&permit, Some(&answers), AttemptStatus::Answered)
		.await
		.unwrap();
	assert!(matches!(
		journal
			.finish_attempt(&permit, None, AttemptStatus::Failed)
			.await,
		Err(aidash_application::Error::Conflict(_))
	));
	let recovered = journal
		.recover(first.decision, &first.boundary)
		.await
		.unwrap();
	assert_eq!(recovered.len(), 1);
	assert_eq!(recovered[0].status, AttemptStatus::Answered);
	assert_eq!(recovered[0].answers.as_ref(), Some(&answers));
	// The call limit pauses the Run instead of charging a third attempt.
	let mut second = record(&f.store, &run, worker, &decider, &["q2_call"], 'a');
	second.decision = Uuid::new_v4();
	assert!(matches!(
		journal.reserve(&second).await.unwrap(),
		Reservation::Fresh(_)
	));
	let mut third = record(&f.store, &run, worker, &decider, &["q3_call"], 'a');
	third.decision = Uuid::new_v4();
	assert!(matches!(
		journal.reserve(&third).await,
		Err(aidash_application::Error::Context(
			Failure::PruneUnavailable
		))
	));
	assert_eq!(count(&f.store, "decision_attempts", run.id).await, 2);
	f.store.release_lease(run.id, worker).await.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn every_charge_rechecks_the_current_allowance(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (run, worker) = leased(&f, &app, &token).await;
	let decider = bound(&f.store.node_id, Mode::Enforce);
	let policy = Policy::new(allowance(&decider, 10));
	let journal =
		RunDecisionJournal::new(&f.store, run.clone(), allowance(&decider, 10), &policy).unwrap();
	let charge = |question: &str| {
		let mut charged = record(&f.store, &run, worker, &decider, &[question], 'a');
		charged.decision = Uuid::new_v4();
		charged
	};
	// Arrange: one attempt is charged under the admitted limit.
	assert!(matches!(
		journal.reserve(&charge("q1_call")).await.unwrap(),
		Reservation::Fresh(_)
	));
	// Act + Assert: a current limit lowered below the calls made is already spent.
	policy.set(Some(allowance(&decider, 1)));
	assert!(matches!(
		journal.reserve(&charge("q2_call")).await,
		Err(aidash_application::Error::Context(
			Failure::PruneUnavailable
		))
	));
	// A Decider the current policy no longer allows is unavailable.
	policy.set(None);
	assert!(matches!(
		journal.reserve(&charge("q3_call")).await,
		Err(aidash_application::Error::Context(
			Failure::PruneUnavailable
		))
	));
	// A current policy that pins another Decider never charges the admitted one.
	let mut repinned = allowance(&decider, 10);
	repinned.decider.provider_implementation = "node-decision-adapter:other".into();
	policy.set(Some(repinned));
	assert!(matches!(
		journal.reserve(&charge("q4_call")).await,
		Err(aidash_application::Error::Invalid(_))
	));
	assert_eq!(count(&f.store, "decision_attempts", run.id).await, 1);
	// Restoring the policy charges again.
	policy.set(Some(allowance(&decider, 10)));
	assert!(matches!(
		journal.reserve(&charge("q5_call")).await.unwrap(),
		Reservation::Fresh(_)
	));
	assert_eq!(count(&f.store, "decision_attempts", run.id).await, 2);
	f.store.release_lease(run.id, worker).await.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_lost_lease_or_stale_revision_is_a_conflict_and_recovery_keeps_the_original_fence(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (run, worker) = leased(&f, &app, &token).await;
	let decider = bound(&f.store.node_id, Mode::Enforce);
	let policy = Policy::new(allowance(&decider, 10));
	let journal =
		RunDecisionJournal::new(&f.store, run.clone(), allowance(&decider, 10), &policy).unwrap();
	let mut charged = record(&f.store, &run, worker, &decider, &["q1_call"], 'e');
	charged.decision = Uuid::new_v4();
	let mut stranger = charged.clone();
	stranger.boundary.worker = Uuid::new_v4();
	assert!(matches!(
		journal.reserve(&stranger).await,
		Err(aidash_application::Error::Conflict(_))
	));
	let mut stale = charged.clone();
	stale.boundary.run_revision += 1;
	assert!(matches!(
		journal.reserve(&stale).await,
		Err(aidash_application::Error::Conflict(_))
	));
	let Reservation::Fresh(permit) = journal.reserve(&charged).await.unwrap() else {
		panic!("a new attempt is charged");
	};
	// Act: the lease moves to another worker before the attempt finishes.
	f.store.release_lease(run.id, worker).await.unwrap();
	let successor = Uuid::new_v4();
	let leased = f
		.store
		.lease_run(successor, 60)
		.await
		.unwrap()
		.expect("leased Run");
	let journal =
		RunDecisionJournal::new(&f.store, leased.clone(), allowance(&decider, 10), &policy)
			.unwrap();
	let mut resumed = charged.clone();
	resumed.boundary.worker = successor;
	resumed.boundary.run_revision = leased.revision;
	let recovered = journal
		.recover(charged.decision, &resumed.boundary)
		.await
		.unwrap();
	// Assert: the attempt keeps its original worker fence and is never re-charged.
	assert_eq!(recovered.len(), 1);
	assert_eq!(recovered[0].permit, permit);
	assert_eq!(recovered[0].status, AttemptStatus::Uncertain);
	assert!(matches!(
		journal.reserve(&resumed).await.unwrap(),
		Reservation::Recovered(_)
	));
	// A late result of the old worker is still recorded.
	journal
		.finish_attempt(&permit, None, AttemptStatus::Uncertain)
		.await
		.unwrap();
	assert!(matches!(
		journal
			.append_outcome(&OutcomeLink {
				decision: charged.decision,
				next_inference: None,
				observed_status: "paused".into(),
			})
			.await,
		Err(aidash_application::Error::NotFound(_))
	));
	f.store.release_lease(run.id, successor).await.unwrap();
	cleanup(f, &url, &schema).await;
}
