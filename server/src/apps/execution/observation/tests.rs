//! PostgreSQL tests own observer lease/charge/label/isolation invariants.
use super::{journal::Journal, repository};
use crate::apps::execution::test_database::{DatabaseFixture, database};
use aidash_application::{
	decision::observation::{ObservationJournal, ObservationLease, observation_boundary},
	ports::decision::{DecisionJournal, DispatchRecord, Reservation},
	tenancy::TenancyScope,
};
use aidash_domain::{
	decision::{
		observation::{report::ReportRange, *},
		*,
	},
	registry::{Entry, bindings::QualifiedRef, rules::digest},
};
use chrono::{Duration, Utc};
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::json;
use uuid::Uuid;

fn frame(tenant: &str) -> ObservationFrame {
	let contract = Hook::ApprovalObservation.contract();
	let config = DeciderConfig {
		hook: Hook::ApprovalObservation,
		answer_type: AnswerType::Noul,
		description: "Observe actual human approvals".into(),
		provider_credential: "openrouter".into(),
		routes: vec![DeciderRoute::jev("typesafe/jev-1.13")],
		builder: contract.builder.into(),
		option_source: contract.option_source.into(),
		rule: contract.rule.into(),
		mode: Mode::Shadow,
		max_calls_per_run: 1,
		state_retention: Default::default(),
	};
	let definition:Entry=serde_json::from_value(json!({"id":"observer","version":"1.0.0","kind":"decider","name":{"en":"Observer"},"description":{"en":"Independent observer"},"config":config})).unwrap();
	let approval = Uuid::new_v4();
	let task = Uuid::new_v4();
	let pending = PendingAction::Generation {
		request: PendingGeneration {
			request: approval,
			task,
			policy_id: "policy".into(),
			policy_revision: 1,
			requested_agent_id: "generated".into(),
			requested_agent_version: "1.0.0".into(),
			definition: json!({"instructions":"Read docs"}),
			token_limit: 1000,
			depth: 1,
		},
	};
	let mut frame = ObservationFrame {
		version: 1,
		id: Uuid::nil(),
		tenant: tenant.into(),
		approval,
		node: "aidash://observer".into(),
		created_at: Utc::now(),
		kind: ApprovalKind::Generation,
		origin: ObservationOrigin::Generation {
			request: approval,
			task,
			policy_id: "policy".into(),
			policy_revision: 1,
		},
		pending_digest: pending.digest().unwrap(),
		settings: ObserverSettings {
			revision: 1,
			decider: Some(DeciderPin {
				identity: QualifiedRef {
					registry_deployment: "aidash://observer".into(),
					id: definition.id.clone(),
					version: definition.version.clone(),
				},
				definition_digest: digest(&json!(definition)),
				configuration_digest: config.digest().unwrap(),
				provider_implementation: "observer-fixture".into(),
			}),
			config: Some(config),
			restrictions: Default::default(),
		},
		input_cutoff: 0,
		input_refs: vec![],
		instruction_ref: None,
		action_ref: repository::reference(format!("generation:{approval}"), &json!(pending)),
		policy_refs: vec![],
		prior_context_refs: vec![],
		operation_class: None,
		effect_class: Some("generation".into()),
	};
	frame.id = frame.identity();
	frame.validate().unwrap();
	frame
}
async fn insert(store: &crate::store::Store, frame: &ObservationFrame, lease: &ObservationLease) {
	let mut tx = store
		.pool
		.begin(&TenancyScope::tenant(&frame.tenant).unwrap())
		.await
		.unwrap();
	crate::database::native::query(
		&Query::insert()
			.into_table(Alias::new("approval_observations"))
			.columns(
				[
					"id",
					"tenant",
					"approval",
					"approval_type",
					"workspace_id",
					"credential_id",
					"subject",
					"frame",
					"settlement",
					"worker",
					"lease_revision",
					"leased_until",
					"created_at",
				]
				.map(Alias::new),
			)
			.values_panic::<_, reinhardt::query::Value>([
				frame.id.into(),
				frame.tenant.clone().into(),
				frame.approval.into(),
				"generation".into(),
				Uuid::new_v4().into(),
				Uuid::new_v4().into(),
				"alice".into(),
				json!(frame).into(),
				"pending".into(),
				lease.worker.into(),
				lease.revision.into(),
				(Utc::now() + Duration::minutes(10)).into(),
				frame.created_at.into(),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	crate::database::native::query(
		&Query::insert()
			.into_table(Alias::new("approval_observation_population"))
			.columns(
				[
					"approval",
					"tenant",
					"approval_type",
					"kind",
					"settlement",
					"created_at",
				]
				.map(Alias::new),
			)
			.values_panic::<_, reinhardt::query::Value>([
				frame.approval.into(),
				frame.tenant.clone().into(),
				"generation".into(),
				"generation".into(),
				"pending".into(),
				frame.created_at.into(),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
}
fn record(
	frame: &ObservationFrame,
	lease: &ObservationLease,
	decision: Uuid,
	request: &str,
) -> DispatchRecord {
	let questions: Vec<_> = observation_questions().into_keys().collect();
	let request_digest = digest(&json!(request));
	let boundary = observation_boundary(frame, lease).unwrap();
	let state_digest = digest(&json!({"fixture":decision,"request":request}));
	let decision = aidash_application::decision::decision_identity(
		&boundary,
		frame.settings.decider.as_ref().unwrap(),
		&frame.settings.restrictions,
		&state_digest,
		&[],
		0,
		0,
	);
	DispatchRecord {
		attempt: aidash_application::decision::attempt_identity(
			decision,
			&request_digest,
			&questions,
		),
		decision,
		boundary,
		decider: frame.settings.decider.clone().unwrap(),
		binding_restrictions: frame.settings.restrictions.clone(),
		route: 0,
		request_digest,
		state_digest,
		questions: questions.clone(),
		question_digest: aidash_domain::decision::question_digest(
			&observation_questions(),
			&questions,
		)
		.unwrap(),
		mode: Mode::Shadow,
	}
}
#[rstest::rstest]
#[tokio::test]
async fn native_observation_recovery_labels_and_tenant_isolation(
	#[future] database: DatabaseFixture,
) {
	let db = database.await;
	let scope = db.tenant("observer-a").await;
	let other = db.tenant("observer-b").await;
	let store = crate::store::Store::from_pool(
		db.connection.clone().into_postgres().unwrap(),
		aidash_domain::nodes::DeploymentId::parse("aidash://observer").unwrap(),
	)
	.await
	.unwrap();
	// Disabled means no observer work or provider attempt, and still has a report denominator.
	let mut tx = store.pool.begin(&scope).await.unwrap();
	assert!(
		repository::settings(&mut *tx, "observer-a")
			.await
			.unwrap()
			.decider
			.is_none()
	);
	tx.commit().await.unwrap();
	let frame = frame("observer-a");
	let lease = ObservationLease {
		worker: Uuid::new_v4(),
		revision: 1,
	};
	insert(&store, &frame, &lease).await;
	let journal = Journal {
		store: store.clone(),
		frame: frame.clone(),
		lease: lease.clone(),
	};
	let dispatch = record(&frame, &lease, Uuid::new_v4(), "first");
	let (first, duplicate) = tokio::join!(journal.reserve(&dispatch), journal.reserve(&dispatch));
	let reservations = [first.unwrap(), duplicate.unwrap()];
	assert_eq!(
		reservations
			.iter()
			.filter(|r| matches!(r, Reservation::Fresh(_)))
			.count(),
		1
	);
	assert_eq!(
		reservations
			.iter()
			.filter(|r| matches!(r,Reservation::Recovered(r) if r.status==AttemptStatus::Uncertain))
			.count(),
		1
	);
	assert!(matches!(
		journal
			.reserve(&record(&frame, &lease, Uuid::new_v4(), "second"))
			.await,
		Err(aidash_application::Error::Context(
			aidash_domain::context::recovery::Failure::DeciderCallsExhausted
		))
	));
	// Human response arrives first. Observer completion preserves it and never settles the approval.
	let evidence = ObservationEvidence {
		observation: frame.id,
		frame_digest: digest(&json!(frame)),
		outcome: ObservationOutcome::Unavailable {
			reason: UnavailableReason::CredentialUnavailable,
		},
		routes: vec![],
		completed_at: Utc::now(),
	};
	let mut tx = store.pool.begin(&scope).await.unwrap();
	repository::label(&mut *tx, "observer-a", frame.approval, HumanLabel::Allow)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	journal
		.commit_observation(&frame, &lease, &evidence)
		.await
		.unwrap();
	journal
		.commit_observation(&frame, &lease, &evidence)
		.await
		.unwrap();
	let mut tx = store.pool.begin(&scope).await.unwrap();
	let stored = repository::observation(&mut *tx, "observer-a", frame.approval)
		.await
		.unwrap()
		.unwrap();
	assert_eq!(stored.settlement, "human_allow");
	repository::settlement(
		&mut *tx,
		"observer-a",
		frame.approval,
		SettlementClass::Expired,
	)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	let mut tx = store.pool.begin(&other).await.unwrap();
	assert!(
		repository::observation(&mut *tx, "observer-a", frame.approval)
			.await
			.unwrap()
			.is_none()
	);
	assert!(
		repository::settings(&mut *tx, "observer-a")
			.await
			.unwrap()
			.decider
			.is_none()
	);
	tx.commit().await.unwrap();
	// A replacement worker recovers the already charged attempt; the old lease loses every write.
	let new_lease = ObservationLease {
		worker: Uuid::new_v4(),
		revision: 2,
	};
	let mut tx = store.pool.begin(&scope).await.unwrap();
	crate::database::native::query(
		&Query::update()
			.table(Alias::new("approval_observations"))
			.value(Alias::new("worker"), new_lease.worker)
			.value(Alias::new("lease_revision"), new_lease.revision)
			.and_where(Expr::col("id").eq(Expr::value(frame.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	assert!(
		journal
			.recover(dispatch.decision, &dispatch.boundary)
			.await
			.is_err()
	);
	let recovered = Journal {
		store: store.clone(),
		frame: frame.clone(),
		lease: new_lease.clone(),
	};
	let recovered = recovered
		.recover(
			dispatch.decision,
			&observation_boundary(&frame, &new_lease).unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(recovered.len(), 1);
	assert_eq!(recovered[0].status, AttemptStatus::Uncertain);
	let mut tx = store.pool.begin(&scope).await.unwrap();
	// Approval identity includes its type. A colliding UUID of another type
	// must neither multiply this denominator nor replace its observation.
	let columns = [
		"run_id",
		"workspace_id",
		"credential_id",
		"subject",
		"frame",
		"evidence",
		"settlement",
		"worker",
		"lease_revision",
		"leased_until",
		"created_at",
	];
	let copy = Query::select()
		.expr(Expr::value(Uuid::new_v4()))
		.columns(["tenant", "approval"].map(Alias::new))
		.expr(Expr::value("capability"))
		.columns(columns.map(Alias::new))
		.from(Alias::new("approval_observations"))
		.and_where(Expr::col("id").eq(Expr::value(frame.id)))
		.to_owned();
	crate::database::native::query(
		&Query::insert()
			.into_table(Alias::new("approval_observations"))
			.columns(
				["id", "tenant", "approval", "approval_type"]
					.into_iter()
					.chain(columns)
					.map(Alias::new),
			)
			.from_subquery(copy)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	let report = repository::report_read(
		&mut tx,
		"observer-a",
		ReportRange {
			start: frame.created_at - Duration::seconds(1),
			end: Utc::now() + Duration::seconds(1),
		},
	)
	.await
	.unwrap();
	assert_eq!(report.metrics.total_approvals, 1);
	assert_eq!(report.metrics.explicit_labels, 1);
	assert_eq!(report.metrics.exclusions.unavailable_observations, 1);
	assert_eq!(report.metrics.comparable, 0);
	assert_eq!(report.metrics.agreement.value, None);
	tx.commit().await.unwrap();
}

struct FixtureAuthority;
#[async_trait::async_trait]
impl aidash_application::ports::decision::DecisionAuthority for FixtureAuthority {
	async fn check(
		&self,
		_: &Boundary,
		_: &DeciderPin,
		_: &[SourcePin],
	) -> aidash_application::Result<aidash_application::ports::decision::Approval> {
		Ok(aidash_application::ports::decision::Approval {
			restrictions: Default::default(),
			state_retention: Default::default(),
		})
	}
	async fn read(&self, _: &Evidence, _: bool) -> aidash_application::Result<()> {
		Err(aidash_application::Error::Forbidden)
	}
}
struct FixtureProvider {
	config: DeciderConfig,
	calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
struct FixtureDispatch {
	calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
	questions: Vec<String>,
}
#[async_trait::async_trait]
impl aidash_application::ports::decision::DecisionProvider for FixtureProvider {
	fn implementation_id(&self) -> &str {
		"observer-fixture"
	}
	fn configuration_digest(&self) -> aidash_application::Result<String> {
		Ok(self.config.digest()?)
	}
	fn plan(
		&self,
		_: usize,
		state: &serde_json::Value,
		questions: &Questions,
	) -> aidash_application::Result<Vec<aidash_application::ports::decision::PreparedRequest>> {
		Ok(vec![aidash_application::ports::decision::PreparedRequest {
			body: serde_json::to_vec(&json!({"state":state,"questions":questions}))?,
			questions: questions.clone(),
		}])
	}
	async fn prepare(
		&self,
		_: usize,
		request: &aidash_application::ports::decision::PreparedRequest,
		_: Uuid,
	) -> aidash_application::Result<
		Box<dyn aidash_application::ports::decision::PreparedDispatch + '_>,
	> {
		Ok(Box::new(FixtureDispatch {
			calls: self.calls.clone(),
			questions: request.questions.keys().cloned().collect(),
		}))
	}
}
#[async_trait::async_trait]
impl aidash_application::ports::decision::PreparedDispatch for FixtureDispatch {
	async fn dispatch(
		self: Box<Self>,
	) -> std::result::Result<
		std::collections::BTreeMap<String, Probability>,
		aidash_application::ports::decision::DispatchError,
	> {
		self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
		Ok(self
			.questions
			.iter()
			.map(|q| (q.clone(), Probability::new(0.01).unwrap()))
			.collect())
	}
	async fn dispatch_reported(
		self: Box<Self>,
	) -> aidash_application::ports::decision::ReportedDispatch {
		let result = self.dispatch().await;
		aidash_application::ports::decision::ReportedDispatch {
			result,
			provider_report: AttemptProviderReport::Reported {
				served_model: Some("typesafe/jev-1.13".into()),
				served_host: Some("typesafe".into()),
				input_tokens: Some(128),
				output_tokens: Some(16),
				cost_usd: Some(ProviderCost::new(0.0).unwrap()),
			},
		}
	}
}
#[rstest::rstest]
#[tokio::test]
async fn native_observer_gate_dispatches_once_and_replays_durable_provider_facts(
	#[future] database: DatabaseFixture,
) {
	use aidash_application::decision::{
		DecisionGate,
		observation::{ObservationInput, ObservationRequest},
	};
	let db = database.await;
	db.tenant("observer-gate").await;
	let store = crate::store::Store::from_pool(
		db.connection.clone().into_postgres().unwrap(),
		aidash_domain::nodes::DeploymentId::parse("aidash://observer").unwrap(),
	)
	.await
	.unwrap();
	let frame = frame("observer-gate");
	let lease = ObservationLease {
		worker: Uuid::new_v4(),
		revision: 1,
	};
	insert(&store, &frame, &lease).await;
	let journal = Journal {
		store: store.clone(),
		frame: frame.clone(),
		lease: lease.clone(),
	};
	let ObservationOrigin::Generation {
		request,
		task,
		policy_revision,
		ref policy_id,
	} = frame.origin
	else {
		panic!("real generation origin")
	};
	let input = ObservationInput {
		inputs: vec![],
		instructions: None,
		policies: vec![],
		prior_calls: vec![],
		pending: PendingAction::Generation {
			request: PendingGeneration {
				request,
				task,
				policy_id: policy_id.clone(),
				policy_revision,
				requested_agent_id: "generated".into(),
				requested_agent_version: "1.0.0".into(),
				definition: json!({"instructions":"Read docs"}),
				token_limit: 1000,
				depth: 1,
			},
		},
	};
	let definition:Entry=serde_json::from_value(json!({"id":"observer","version":"1.0.0","kind":"decider","name":{"en":"Observer"},"description":{"en":"Independent observer"},"config":frame.settings.config})).unwrap();
	let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
	let provider = FixtureProvider {
		config: frame.settings.config.clone().unwrap(),
		calls: calls.clone(),
	};
	let gate = DecisionGate {
		authority: &FixtureAuthority,
		journal: &journal,
		provider: &provider,
	};
	let requested = ObservationRequest {
		frame: &frame,
		lease: &lease,
		input: Some(&input),
		definition: Some(&definition),
		now: Utc::now(),
	};
	let evidence = gate.observe_approval(&requested, &journal).await.unwrap();
	assert!(
		matches!(
			evidence.outcome,
			ObservationOutcome::Available {
				verdict: Verdict::Allowed
			}
		),
		"{evidence:?}"
	);
	assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
	assert_eq!(evidence.routes[0].attempts.len(), 1);
	assert!(!evidence.routes[0].attempts[0].owner_receipts.is_empty());
	let report = &evidence.routes[0].attempts[0].provider_report;
	assert!(
		matches!(report,AttemptProviderReport::Reported{input_tokens:Some(128),output_tokens:Some(16),cost_usd:Some(cost),..} if cost.value()==0.0)
	);
	assert_eq!(
		gate.observe_approval(&requested, &journal).await.unwrap(),
		evidence
	);
	assert_eq!(
		calls.load(std::sync::atomic::Ordering::SeqCst),
		1,
		"completed recovery never dispatches or charges again"
	);
	let attempt: &AttemptEvidence = &evidence.routes[0].attempts[0];
	let stored = journal
		.recover(
			aidash_application::decision::decision_identity(
				&observation_boundary(&frame, &lease).unwrap(),
				frame.settings.decider.as_ref().unwrap(),
				&frame.settings.restrictions,
				&evidence.routes[0].state_digest,
				&[],
				0,
				0,
			),
			&observation_boundary(&frame, &lease).unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(stored.len(), 1);
	assert_eq!(stored[0].provider_report, attempt.provider_report);
}
