//! Decision Evidence and Decision Attempts persist behind the portable gate:
//! charged attempts are recovered, never charged twice, and an applied decision
//! commits its context and evidence atomically under the Run's lease.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "support/legacy.rs"]
mod common;

use aidash_application::{
	decision::{
		ApprovalTool, CompactionResult, DecisionGate, Evaluation, Judgement, Ruling,
		approval_input_digest, compaction_input_digest,
	},
	ports::decision::{
		Approval, DecisionAuthority, DecisionJournal, DecisionProvider, Disclosure, DispatchError,
		DispatchRecord, PreparedDispatch, PreparedRequest, Reservation, RetainedState,
	},
	tenancy::TEST_FIXTURE,
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
		provider_credential: "openrouter".into(),
		routes: vec![DeciderRoute::jev("typesafe/jev-1.13")],
		builder: BUILDER.into(),
		option_source: OPTION_SOURCE.into(),
		rule: RULE.into(),
		mode,
		max_calls_per_run: 1_000,
		state_retention: StateRetention::default(),
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
	/// Every request fails as an outage.
	outage: AtomicBool,
}
impl Provider {
	fn new(mode: Mode) -> Self {
		Self {
			digest: config(mode).digest().unwrap(),
			keep: Probability::new(0.0).unwrap(),
			calls: AtomicUsize::new(0),
			outage: AtomicBool::new(false),
		}
	}
	/// The provider of the approval Decider, which answers every question with
	/// probability `answer`.
	fn approving(mode: Mode, answer: f64) -> Self {
		Self {
			digest: approval_config(mode).digest().unwrap(),
			keep: Probability::new(answer).unwrap(),
			..Self::new(mode)
		}
	}
}
#[async_trait]
impl DecisionProvider for Provider {
	fn implementation_id(&self) -> &str {
		IMPLEMENTATION
	}
	fn configuration_digest(&self) -> aidash_application::Result<String> {
		Ok(self.digest.clone())
	}
	fn plan(
		&self,
		_: usize,
		state: &Value,
		questions: &Questions,
	) -> aidash_application::Result<Vec<PreparedRequest>> {
		Ok(vec![PreparedRequest {
			body: serde_json::to_vec(&json!({"state":state,"questions":questions})).unwrap(),
			questions: questions.clone(),
		}])
	}
	async fn prepare(
		&self,
		_: usize,
		request: &PreparedRequest,
		_: Uuid,
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
		if self.provider.outage.load(Ordering::SeqCst) {
			return Err(aidash_application::Error::External("route outage".into()).into());
		}
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
/// `revoke` is the journal's current policy, which loses its Decider once the
/// provider has answered.
struct Authority<'a> {
	provider: &'a Provider,
	forbid_after_dispatch: AtomicBool,
	retain_state: bool,
	revoke: Option<&'a Policy>,
}
#[async_trait]
impl DecisionAuthority for Authority<'_> {
	async fn check(
		&self,
		_: &Boundary,
		_: &DeciderPin,
		_: &[SourcePin],
	) -> aidash_application::Result<Approval> {
		let answered = self.provider.calls.load(Ordering::SeqCst) > 0;
		let forbid = self.forbid_after_dispatch.load(Ordering::SeqCst) && answered;
		if let Some(policy) = self.revoke
			&& answered
		{
			policy.set(None);
		}
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
		.save_run(&TEST_FIXTURE, &run, worker, "run.sources_observed")
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
	boundary_in(store, run, worker, context, WINDOW)
}

fn boundary_in(
	store: &Store,
	run: &Run,
	worker: Uuid,
	context: &Context,
	window: usize,
) -> Boundary {
	let projection = RequestProjection::Legacy;
	let budget = RequestBudget {
		window,
		..budget(&projection)
	};
	Boundary {
		node: store.node_id.clone(),
		run: run.id,
		step: run.step,
		run_revision: run.revision,
		input_revision: run.observed_input_seq,
		worker,
		input_digest: compaction_input_digest(context, &budget, &json!({})).unwrap(),
		cycle: 0,
		retry: 0,
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
	decide_in(store, run, worker, mode, authority, provider, WINDOW).await
}

/// `decide` against a model window of `window` tokens.
async fn decide_in(
	store: &Store,
	run: &Run,
	worker: Uuid,
	mode: Mode,
	authority: &Authority<'_>,
	provider: &Provider,
	window: usize,
) -> (aidash_application::Result<CompactionResult>, Context) {
	let decider = bound(&store.node_id, mode);
	let admitted = Policy::new(allowance(&decider, 10));
	let policy = authority.revoke.unwrap_or(&admitted);
	let journal =
		RunDecisionJournal::new(store, run.clone(), allowance(&decider, 10), policy).unwrap();
	let mut context = run.context.clone();
	let boundary = boundary_in(store, run, worker, &context, window);
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
	let budget = RequestBudget {
		window,
		..budget(&projection)
	};
	let result = gate
		.compact(&mut context, &budget, &json!({}), &input)
		.await;
	(result, context)
}

/// Whether `message` refuses evidence as inconsistent with its charged
/// attempts, its replay or its answers, whichever check finds it first.
fn inconsistent(message: &str) -> bool {
	[
		"charged attempts",
		"historical",
		"decision answers must cover",
	]
	.iter()
	.any(|reason| message.contains(reason))
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
	.fetch_all(store.pool.fixture_driver())
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
	.fetch_one(store.pool.fixture_driver())
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
			cycle: 0,
			retry: 0,
		},
		decider: decider.pin.clone(),
		binding_restrictions: Restrictions::default(),
		route: 0,
		request_digest: format!("sha256:{}", request.to_string().repeat(64)),
		state_digest: format!("sha256:{}", "d".repeat(64)),
		questions: questions.iter().map(|q| (*q).to_owned()).collect(),
		question_digest: format!("sha256:{}", "9".repeat(64)),
		mode: Mode::Enforce,
	}
}

/// Key `record` by the stable attempt identity of its decision, request and
/// questions, as the journal requires before it charges anything.
fn seal(mut record: DispatchRecord) -> DispatchRecord {
	record.attempt = aidash_application::decision::attempt_identity(
		record.decision,
		&record.request_digest,
		&record.questions,
	);
	record
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
		revoke: None,
	};
	// Act
	let (result, context) =
		decide(&f.store, &run, worker, Mode::Enforce, &authority, &provider).await;
	// Assert: the decision applied, and the saved Run holds the same context.
	assert!(matches!(result.unwrap(), CompactionResult::Applied(_)));
	assert!(context.history.len() < run.context.history.len());
	let saved = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
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
		revoke: None,
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
		Err(aidash_application::Error::Invalid(message)) if inconsistent(&message)
	));
	// So is evidence that reports a durably answered attempt with another status.
	let (_, mut relabeled) = evidence_rows(&f.store, run.id).await.remove(0);
	relabeled.attempts[0].status = AttemptStatus::Uncertain;
	assert!(matches!(
		journal.commit(&relabeled, None, None).await,
		Err(aidash_application::Error::Invalid(message)) if inconsistent(&message)
	));
	// Evidence must name every charged attempt.
	let (_, mut omitting) = evidence_rows(&f.store, run.id).await.remove(0);
	omitting.attempts.clear();
	assert!(matches!(
		journal.commit(&omitting, None, None).await,
		Err(aidash_application::Error::Invalid(message)) if inconsistent(&message)
	));
	// This rejection selected branches, so it replays in full; it also carries
	// only what its charges recorded: no unbacked answer, no other input or mode.
	let (_, saved) = evidence_rows(&f.store, run.id).await.remove(0);
	let mut unbacked = saved.clone();
	unbacked
		.answers
		.insert("unanswered".into(), Probability::new(1.0).unwrap());
	let mut other_input = saved.clone();
	other_input.boundary.input_digest = format!("sha256:{}", "b".repeat(64));
	let mut other_mode = saved.clone();
	other_mode.mode = Mode::Shadow;
	let mut uncatalogued = saved.clone();
	uncatalogued
		.questions
		.remove(&saved.attempts[0].questions[0]);
	for differing in [unbacked, other_input, other_mode, uncatalogued] {
		assert!(matches!(
			journal.commit(&differing, None, None).await,
			Err(aidash_application::Error::Invalid(message)) if inconsistent(&message)
		));
	}
	// Nor does it skip the invariants every rejection keeps: no fitting reason,
	// no unsupported version and no restrictions looser than the policy floor.
	let mut fitting = saved.clone();
	fitting.reason = Reason::Fits;
	// Only a complete evaluation finds the context too large, so this replays
	// and its reason disagrees with the restrictions and fit metrics.
	let mut mislabeled = saved.clone();
	mislabeled.reason = Reason::Insufficient;
	let mut unsupported = saved.clone();
	unsupported.version = EVIDENCE_VERSION + 1;
	let mut looser = saved.clone();
	looser.restrictions.preserve_recent = 5;
	for (invalid, expected) in [
		(fitting, "corrupt historical application outcome"),
		(mislabeled, "corrupt historical application outcome"),
		(unsupported, "unsupported or incomplete"),
		(looser, "looser than the admitted Binding"),
	] {
		assert!(matches!(
			journal.commit(&invalid, None, None).await,
			Err(aidash_application::Error::Invalid(message)) if message.contains(expected)
		));
	}
	let mut anonymous = saved.clone();
	anonymous.id = Uuid::nil();
	// Its replay or the identity check refuses it, whichever runs first.
	assert!(matches!(
		journal.commit(&anonymous, None, None).await,
		Err(aidash_application::Error::Invalid(message))
			if message.contains("decision identity") || message.contains("unsupported or incomplete")
	));
	// An expired marker must match its retention witness, which the rejection
	// invariants already check.
	let (_, mut marked) = evidence_rows(&f.store, run.id).await.remove(0);
	marked.state = StateReference::Expired {
		id: Uuid::new_v4(),
		digest: marked.state_digest.clone(),
		expired_at: Utc::now(),
	};
	marked.state_retention = StateRetentionWitness::Enabled {
		id: Uuid::new_v4(),
		expires_at: Utc::now() + Duration::days(1),
	};
	assert!(matches!(
		journal.commit(&marked, None, None).await,
		Err(aidash_application::Error::Invalid(message)) if message.contains("retained-state reference changed")
	));
	// A marker whose deadline has not passed by the commit is not an expiry.
	let (_, mut premature) = evidence_rows(&f.store, run.id).await.remove(0);
	let (id, deadline) = (Uuid::new_v4(), Utc::now() + Duration::days(1));
	premature.state = StateReference::Expired {
		id,
		digest: premature.state_digest.clone(),
		expired_at: deadline,
	};
	premature.state_retention = StateRetentionWitness::Enabled {
		id,
		expires_at: deadline,
	};
	assert!(matches!(
		journal.commit(&premature, None, None).await,
		Err(aidash_application::Error::Invalid(message)) if message.contains("retained decision state")
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
	drop(journal);
	let journal =
		RunDecisionJournal::new(&f.store, run.clone(), allowance(&decider, 10), &policy).unwrap();
	let boundary = boundary(&f.store, &run, worker, &run.context);
	assert!(matches!(
		journal.recover(rows[1].1.id, &boundary).await,
		Err(aidash_application::Error::Conflict(_))
	));
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	drop(journal);
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
		revoke: None,
	};
	// Arrange: a Shadow decision keeps its state and leaves the Run unchanged.
	let (result, _) = decide(&f.store, &run, worker, Mode::Shadow, &authority, &provider).await;
	assert!(matches!(result.unwrap(), CompactionResult::Shadow(_)));
	assert_eq!(
		f.store.run(&TEST_FIXTURE, run.id).await.unwrap().revision,
		run.revision
	);
	assert_eq!(count(&f.store, "decision_states", run.id).await, 1);
	let (_, retained) = evidence_rows(&f.store, run.id).await.remove(0);
	let StateReference::Retained {
		id,
		digest,
		expires_at,
	} = retained.state.clone()
	else {
		panic!("a Shadow decision retains its state");
	};
	// State bytes that do not match their digest are never retained.
	let decider = bound(&f.store.node_id, Mode::Shadow);
	let policy = Policy::new(allowance(&decider, 10));
	let journal =
		RunDecisionJournal::new(&f.store, run.clone(), allowance(&decider, 10), &policy).unwrap();
	let foreign = RetainedState {
		id,
		digest,
		expires_at,
		state: json!({"another":"evaluation"}),
	};
	assert!(matches!(
		journal.commit(&retained, None, Some(&foreign)).await,
		Err(aidash_application::Error::Invalid(message)) if message.contains("retained decision state")
	));
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
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	drop(journal);
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
	let first = seal(first);
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
	let changed = seal(changed);
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
	let overlapping = seal(overlapping);
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
	// Another batch of the decision is charged only with the decision's inputs.
	let mut sibling = record(&f.store, &run, worker, &decider, &["q2_call"], 'a');
	sibling.decision = first.decision;
	let sibling = seal(sibling);
	let mut other_input = sibling.clone();
	other_input.boundary.input_digest = format!("sha256:{}", "b".repeat(64));
	let mixed_batches = [
		other_input,
		DispatchRecord {
			state_digest: format!("sha256:{}", "b".repeat(64)),
			..sibling.clone()
		},
		DispatchRecord {
			mode: Mode::Shadow,
			..sibling.clone()
		},
		DispatchRecord {
			binding_restrictions: Restrictions {
				preserve_recent: 8,
				..Default::default()
			},
			..sibling.clone()
		},
	];
	for mixed in &mixed_batches {
		assert!(matches!(
			journal.reserve(mixed).await,
			Err(aidash_application::Error::Conflict(_))
		));
	}
	assert_eq!(count(&f.store, "decision_attempts", run.id).await, 1);
	assert!(matches!(
		journal.reserve(&sibling).await.unwrap(),
		Reservation::Fresh(_)
	));
	// The call limit pauses the Run instead of charging a third attempt.
	let mut third = record(&f.store, &run, worker, &decider, &["q3_call"], 'a');
	third.decision = Uuid::new_v4();
	assert!(matches!(
		journal.reserve(&seal(third)).await,
		Err(aidash_application::Error::Context(
			Failure::DeciderCallsExhausted
		))
	));
	assert_eq!(count(&f.store, "decision_attempts", run.id).await, 2);
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	drop(journal);
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
		seal(charged)
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
			Failure::DeciderCallsExhausted
		))
	));
	// A Decider the current policy no longer allows pauses the Run as unapproved.
	policy.set(None);
	assert!(matches!(
		journal.reserve(&charge("q3_call")).await,
		Err(aidash_application::Error::Context(
			Failure::DeciderUnapproved
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
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	drop(journal);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_decider_revoked_before_the_commit_never_retains_state(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (run, worker) = leased(&f, &app, &token).await;
	let provider = Provider::new(Mode::Shadow);
	let decider = bound(&f.store.node_id, Mode::Shadow);
	let policy = Policy::new(allowance(&decider, 10));
	let authority = Authority {
		provider: &provider,
		forbid_after_dispatch: AtomicBool::new(false),
		retain_state: true,
		revoke: Some(&policy),
	};
	// Act: the current policy drops the Decider after the provider answered.
	let (retaining, _) = decide(&f.store, &run, worker, Mode::Shadow, &authority, &provider).await;
	// Assert: nothing is retained or recorded, and the Run is unchanged.
	assert!(matches!(
		retaining,
		Err(aidash_application::Error::Forbidden)
	));
	assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
	assert_eq!(count(&f.store, "decision_states", run.id).await, 0);
	assert_eq!(count(&f.store, "decision_evidence", run.id).await, 0);
	assert_eq!(
		f.store.run(&TEST_FIXTURE, run.id).await.unwrap().revision,
		run.revision
	);
	// Evidence without an effect is still recorded from the charged answers.
	let authority = Authority {
		retain_state: false,
		..authority
	};
	let (recorded, _) = decide(&f.store, &run, worker, Mode::Shadow, &authority, &provider).await;
	assert!(matches!(recorded.unwrap(), CompactionResult::Shadow(_)));
	assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
	assert_eq!(count(&f.store, "decision_evidence", run.id).await, 1);
	assert_eq!(count(&f.store, "decision_states", run.id).await, 0);
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
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
	let charged = seal(charged);
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
	let mut altered = charged.clone();
	altered.boundary.step += 1;
	assert!(matches!(
		journal.reserve(&altered).await,
		Err(aidash_application::Error::Conflict(_))
	));
	let Reservation::Fresh(permit) = journal.reserve(&charged).await.unwrap() else {
		panic!("a new attempt is charged");
	};
	// Act: the lease moves to another worker before the attempt finishes.
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	let successor = Uuid::new_v4();
	let leased = f
		.store
		.lease_run(successor, 60)
		.await
		.unwrap()
		.expect("leased Run");
	drop(journal);
	let journal =
		RunDecisionJournal::new(&f.store, leased.clone(), allowance(&decider, 10), &policy)
			.unwrap();
	let mut resumed = charged.clone();
	resumed.boundary.worker = successor;
	resumed.boundary.run_revision = leased.revision;
	// A journal still holding the earlier Run snapshot never serves the new lease.
	let outdated =
		RunDecisionJournal::new(&f.store, run.clone(), allowance(&decider, 10), &policy).unwrap();
	assert!(matches!(
		outdated.recover(charged.decision, &resumed.boundary).await,
		Err(aidash_application::Error::Conflict(_))
	));
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
	f.store
		.release_lease(&TEST_FIXTURE, run.id, successor)
		.await
		.unwrap();
	drop(journal);
	drop(outdated);
	cleanup(f, &url, &schema).await;
}

/// A window the unprotected history alone already outgrows, so pruning every
/// candidate is not enough.
const NARROW_WINDOW: usize = 1_000;

#[rstest::rstest]
#[tokio::test]
async fn a_staged_decision_is_recorded_only_with_the_summary_adopted_from_it(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	use aidash_domain::context::recovery::{Attempt, Outcome as Settled, Settlement, Stage};
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (run, worker) = leased(&f, &app, &token).await;
	let provider = Provider::new(Mode::Enforce);
	let authority = Authority {
		provider: &provider,
		forbid_after_dispatch: AtomicBool::new(false),
		retain_state: false,
		revoke: None,
	};
	// Arrange: pruning cannot fit the narrow window, so the gate hands the
	// decision back without recording it.
	let (result, unchanged) = decide_in(
		&f.store,
		&run,
		worker,
		Mode::Enforce,
		&authority,
		&provider,
		NARROW_WINDOW,
	)
	.await;
	let Ok(CompactionResult::Insufficient(staged)) = result else {
		panic!("expected a staged decision");
	};
	assert_eq!(unchanged.history.len(), run.context.history.len());
	assert!(staged.pruned.history.len() < run.context.history.len());
	assert!(evidence_rows(&f.store, run.id).await.is_empty());
	let decider = bound(&f.store.node_id, Mode::Enforce);
	let policy = Policy::new(allowance(&decider, 10));
	let journal =
		RunDecisionJournal::new(&f.store, run.clone(), allowance(&decider, 10), &policy).unwrap();
	// A staged decision is never recorded on its own.
	assert!(matches!(
		journal.commit(&staged.evidence, None, None).await,
		Err(aidash_application::Error::Invalid(_))
	));
	// The summary absorbs the oldest pruned entry and is adopted with it.
	let mut adopted = run.clone();
	adopted.context = staged.pruned.clone();
	let absorbed = adopted.context.history.remove(0);
	adopted.context.compactions += 1;
	let attempt = Attempt {
		id: Uuid::new_v4(),
		run_id: run.id,
		stage: Stage::Summary,
		policy_version: "context-recovery/1".into(),
		provider: "summarizer@1.0.0#sha256:fixture".into(),
		source_from_seq: absorbed.seq,
		source_through_seq: absorbed.seq,
		base_revision: run.revision,
		observed_input_seq: run.observed_input_seq,
		before_tokens: 1_000,
	};
	aidash_application::ports::execution::ExecutionStore::begin_compaction(
		&f.store.for_run(TEST_FIXTURE),
		&run,
		worker,
		&attempt,
		2,
	)
	.await
	.unwrap();
	let settlement = Settlement {
		outcome: Settled::Adopted,
		reason: None,
		candidate_digest: Some("sha256:candidate".into()),
		after_tokens: Some(100),
	};
	// A summary that does not continue from the pruned projection is refused.
	let mut unrelated = adopted.clone();
	unrelated.context.history = run.context.history.clone();
	assert!(matches!(
		f.store
			.adopt_compaction(
				&TEST_FIXTURE,
				&unrelated,
				worker,
				attempt.id,
				&settlement,
				Some((&journal, &staged)),
			)
			.await,
		Err(aidash_server::Error::Invalid(_))
	));
	assert!(evidence_rows(&f.store, run.id).await.is_empty());
	// Act
	f.store
		.adopt_compaction(
			&TEST_FIXTURE,
			&adopted,
			worker,
			attempt.id,
			&settlement,
			Some((&journal, &staged)),
		)
		.await
		.unwrap();
	// Assert: the staged evidence names the pruned projection and was saved
	// with the adopted summary.
	let rows = evidence_rows(&f.store, run.id).await;
	assert_eq!(rows.len(), 1);
	let (outcome, evidence) = &rows[0];
	assert_eq!(outcome, "staged");
	assert_eq!(evidence.reason, Reason::Insufficient);
	assert_eq!(evidence.replay().unwrap(), evidence.branches);
	assert_eq!(
		evidence.projection_digest,
		Some(pruning::projection_digest(&staged.pruned.history).unwrap())
	);
	let saved = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	assert_eq!(saved.context.history.len(), adopted.context.history.len());
	// A staged decision is final, like an applied one.
	assert!(matches!(
		journal.commit(&staged.rejected(), None, None).await,
		Err(aidash_application::Error::Conflict(_))
	));
	drop(journal);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn evidence_must_attest_to_the_context_and_identity_it_claims(
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
		revoke: None,
	};
	// Arrange: a rejected decision leaves evidence whose answers are charged.
	let (rejected, _) = decide(&f.store, &run, worker, Mode::Enforce, &authority, &provider).await;
	assert!(rejected.is_err());
	let decider = bound(&f.store.node_id, Mode::Enforce);
	let policy = Policy::new(allowance(&decider, 10));
	let journal =
		RunDecisionJournal::new(&f.store, run.clone(), allowance(&decider, 10), &policy).unwrap();
	let (_, saved) = evidence_rows(&f.store, run.id).await.remove(0);
	// Act + Assert: the same decision applied with a context other than the one
	// its branches produced is refused.
	let mut applied = saved.clone();
	applied.outcome = Outcome::Applied;
	applied.reason = Reason::Fits;
	applied.restrictions.forbid_apply = false;
	applied.projection_digest = Some(format!("sha256:{}", "e".repeat(64)));
	assert!(matches!(
		journal.commit(&applied, Some(&run.context), None).await,
		Err(aidash_application::Error::Invalid(message)) if message.contains("applied context differs")
	));
	// Evidence keeps the admitted Binding restrictions its identity commits to,
	// so evidence claiming other ones is not this decision.
	let mut other_binding = saved.clone();
	other_binding.binding_restrictions.preserve_recent = 8;
	other_binding.restrictions.preserve_recent = 8;
	assert!(matches!(
		journal.commit(&other_binding, None, None).await,
		Err(aidash_application::Error::Invalid(_))
	));
	// A charge must use its stable attempt identity and the restriction floor.
	let mut unstable = record(&f.store, &run, worker, &decider, &["q9_call"], 'a');
	unstable.decision = Uuid::new_v4();
	assert!(matches!(
		journal.reserve(&unstable).await,
		Err(aidash_application::Error::Invalid(_))
	));
	let mut loose = seal(unstable);
	loose.binding_restrictions.preserve_recent = 5;
	assert!(matches!(
		journal.reserve(&loose).await,
		Err(aidash_application::Error::Invalid(_))
	));
	assert_eq!(count(&f.store, "decision_attempts", run.id).await, 1);
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	drop(journal);
	cleanup(f, &url, &schema).await;
}

/// The approval Decider's configuration (ADR 0077).
fn approval_config(mode: Mode) -> DeciderConfig {
	DeciderConfig {
		hook: Hook::Approval,
		description: "Block risky calls".into(),
		builder: APPROVAL_BUILDER.into(),
		option_source: APPROVAL_OPTION_SOURCE.into(),
		rule: APPROVAL_RULE.into(),
		mode,
		max_calls_per_run: 200,
		..config(mode)
	}
}

fn approval_definition(mode: Mode) -> Entry {
	serde_json::from_value(
		json!({"id":"approval-decider","version":"1.0.0","kind":"decider",
		"name":{"en":"Approval"},"description":{"en":"Judge calls"},"config":approval_config(mode)}),
	)
	.unwrap()
}

fn approval_bound(node: &str, mode: Mode) -> BoundDecider {
	let definition = approval_definition(mode);
	let config = approval_config(mode);
	BoundDecider {
		pin: DeciderPin {
			identity: QualifiedRef {
				registry_node: node.into(),
				id: definition.id.clone(),
				version: definition.version.clone(),
			},
			definition_digest: digest(&serde_json::to_value(&definition).unwrap()),
			configuration_digest: config.digest().unwrap(),
			provider_implementation: IMPLEMENTATION.into(),
		},
		config,
		restrictions: Restrictions::default(),
	}
}

const APPROVAL: Uuid = Uuid::from_u128(0xa1);

/// Judges the pending call of approval `APPROVAL` against the PostgreSQL journal
/// of `run`, with at most `calls` Decision Attempts for the Run.
async fn judge_call(
	store: &Store,
	run: &Run,
	worker: Uuid,
	mode: Mode,
	provider: &Provider,
	calls: u64,
) -> aidash_application::Result<Ruling> {
	let decider = approval_bound(&store.node_id, mode);
	let journal = RunDecisionJournal::new(
		store,
		run.clone(),
		allowance(&decider, calls),
		Policy::new(allowance(&decider, calls)),
	)
	.unwrap();
	let authority = Authority {
		provider,
		forbid_after_dispatch: AtomicBool::new(false),
		retain_state: false,
		revoke: None,
	};
	let pending = ToolCall {
		id: "pending".into(),
		name: "http_post".into(),
		arguments: json!({"body":"Q3 summary"}),
	};
	let tool = ApprovalTool {
		id: "acme.http_post".into(),
		version: "1.0.0".into(),
		description: "Post a document".into(),
		input_schema: json!({"type":"object"}),
		target: Some("https://api.example.com".into()),
	};
	let people = [aidash_domain::run_input::RunInput {
		seq: 1,
		sender: "alice".into(),
		content: "Post the Q3 summary".into(),
		idempotency_key: "k1".into(),
		message_id: None,
		reference_only: false,
	}];
	let definition = approval_definition(mode);
	let boundary = Boundary {
		node: store.node_id.clone(),
		run: run.id,
		step: run.step,
		run_revision: run.revision,
		input_revision: run.observed_input_seq,
		worker,
		input_digest: approval_input_digest(APPROVAL, &pending, &tool),
		cycle: 0,
		retry: 0,
	};
	DecisionGate {
		authority: &authority,
		journal: &journal,
		provider,
	}
	.judge_approval(&Judgement {
		boundary: &boundary,
		decider: &decider,
		definition: &definition,
		now: Utc::now(),
		approval: APPROVAL,
		context: &run.context,
		people: &people,
		instructions: "Follow the task",
		pending: &pending,
		tool: &tool,
	})
	.await
}

async fn decided(
	store: &Store,
	run: &Run,
	worker: Uuid,
	mode: Mode,
	provider: &Provider,
) -> ApprovalEvidence {
	match judge_call(store, run, worker, mode, provider, 10)
		.await
		.unwrap()
	{
		Ruling::Decided(evidence) => *evidence,
		Ruling::AllowanceSpent => panic!("the allowance is spent"),
	}
}

/// Every approval decision recorded for `run`: its outcome, reason and evidence.
async fn approval_rows(store: &Store, run: Uuid) -> Vec<(String, String, ApprovalEvidence)> {
	use sqlx::Row as _;
	sqlx::query(
		&Query::select()
			.columns(["outcome", "reason", "evidence"].map(Alias::new))
			.from(Alias::new("decision_evidence"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.order_by(Alias::new("created_at"), Order::Asc)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(store.pool.fixture_driver())
	.await
	.unwrap()
	.into_iter()
	.map(|row| {
		(
			row.get("outcome"),
			row.get("reason"),
			serde_json::from_value(row.get::<Value, _>("evidence")).unwrap(),
		)
	})
	.collect()
}

#[rstest::rstest]
#[case::cleared(Mode::Enforce, 0.0, "allowed", "clear")]
#[case::flagged(Mode::Enforce, 0.9, "blocked", "data_exfiltration")]
#[case::shadow(Mode::Shadow, 0.9, "shadow", "data_exfiltration")]
#[tokio::test]
async fn an_approval_decision_is_recorded_once_and_judging_again_reuses_its_charged_answers(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
	#[case] mode: Mode,
	#[case] answer: f64,
	#[case] outcome: &str,
	#[case] reason: &str,
) {
	// Arrange
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (run, worker) = leased(&f, &app, &token).await;
	let provider = Provider::approving(mode, answer);

	// Act
	let first = decided(&f.store, &run, worker, mode, &provider).await;
	let again = decided(&f.store, &run, worker, mode, &provider).await;

	// Assert: one record, one provider call, one charge, and the same decision.
	assert_eq!(first.id, again.id);
	assert_eq!(first.answers, again.answers);
	assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
	assert_eq!(count(&f.store, "decision_attempts", run.id).await, 1);
	let rows = approval_rows(&f.store, run.id).await;
	assert_eq!(rows.len(), 1, "{rows:?}");
	assert_eq!((rows[0].0.as_str(), rows[0].1.as_str()), (outcome, reason));
	assert_eq!(rows[0].2.id, first.id);
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn an_unavailable_decider_is_recorded_as_a_decision_without_a_verdict(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	// Arrange
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (run, worker) = leased(&f, &app, &token).await;
	let provider = Provider::approving(Mode::Enforce, 0.0);
	provider.outage.store(true, Ordering::SeqCst);

	// Act
	let evidence = decided(&f.store, &run, worker, Mode::Enforce, &provider).await;

	// Assert
	assert_eq!(evidence.route, None);
	let rows = approval_rows(&f.store, run.id).await;
	assert_eq!(rows.len(), 1, "{rows:?}");
	assert_eq!(
		(rows[0].0.as_str(), rows[0].1.as_str()),
		("blocked", "unavailable")
	);
	assert_eq!(rows[0].2.attempts.len(), 1, "the charged attempt is named");
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_spent_call_allowance_records_no_decision_and_leaves_the_call_to_a_person(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	// Arrange
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (run, worker) = leased(&f, &app, &token).await;
	let provider = Provider::approving(Mode::Enforce, 0.0);

	// Act: the Run may spend no Decision Attempt.
	let ruling = judge_call(&f.store, &run, worker, Mode::Enforce, &provider, 0)
		.await
		.unwrap();

	// Assert
	assert!(matches!(ruling, Ruling::AllowanceSpent), "{ruling:?}");
	assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
	assert!(approval_rows(&f.store, run.id).await.is_empty());
	assert_eq!(count(&f.store, "decision_attempts", run.id).await, 0);
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn approval_evidence_that_is_not_the_decision_it_names_is_refused(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	// Arrange
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (run, worker) = leased(&f, &app, &token).await;
	let provider = Provider::approving(Mode::Enforce, 0.0);
	let saved = decided(&f.store, &run, worker, Mode::Enforce, &provider).await;
	// Forget the recorded decision, so the journal checks the evidence afresh.
	sqlx::query(
		&Query::delete()
			.from_table(Alias::new("decision_evidence"))
			.and_where(Expr::col("run_id").eq(Expr::value(run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	let decider = approval_bound(&f.store.node_id, Mode::Enforce);
	let journal = RunDecisionJournal::new(
		&f.store,
		run.clone(),
		allowance(&decider, 10),
		Policy::new(allowance(&decider, 10)),
	)
	.unwrap();

	// Act + Assert: each tampering is refused, and nothing is recorded.
	let mut other_state = saved.clone();
	other_state.state_digest = format!("sha256:{}", "e".repeat(64));
	let mut other_approval = saved.clone();
	other_approval.approval = Uuid::from_u128(0xa2);
	let mut unnamed = saved.clone();
	unnamed.attempts.clear();
	let mut unbacked = saved.clone();
	unbacked
		.answers
		.insert("unanswered".into(), Probability::new(1.0).unwrap());
	let mut other_worker = saved.clone();
	other_worker.boundary.worker = Uuid::new_v4();
	for tampered in [other_state, other_approval, unnamed, unbacked, other_worker] {
		assert!(
			journal.commit_approval(&tampered).await.is_err(),
			"{tampered:?}"
		);
	}
	assert!(approval_rows(&f.store, run.id).await.is_empty());
	// The honest evidence is recorded, and recording it again changes nothing.
	journal.commit_approval(&saved).await.unwrap();
	journal.commit_approval(&saved).await.unwrap();
	assert_eq!(approval_rows(&f.store, run.id).await.len(), 1);
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	drop(journal);
	cleanup(f, &url, &schema).await;
}
