//! PostgreSQL tests own observer lease/charge/label/isolation invariants.
use super::{journal::Journal, repository, views};
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
					"settlement",
					"created_at",
				]
				.map(Alias::new),
			)
			.values_panic::<_, reinhardt::query::Value>([
				frame.approval.into(),
				frame.tenant.clone().into(),
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
	DispatchRecord {
		attempt: aidash_application::decision::attempt_identity(
			decision,
			&request_digest,
			&questions,
		),
		decision,
		boundary: observation_boundary(frame, lease).unwrap(),
		decider: frame.settings.decider.clone().unwrap(),
		binding_restrictions: frame.settings.restrictions.clone(),
		route: 0,
		request_digest,
		state_digest: digest(&json!("original state")),
		questions,
		question_digest: digest(&json!(observation_questions())),
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
	let report = views::report_read(
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
