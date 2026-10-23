#[path = "run_fixtures.rs"]
mod run_fixtures;
use super::Store;
use crate::apps::execution::models::{
	Run as Record,
	states::{RunControl, RunPhase},
};
use crate::apps::execution::tests::native_database::{DatabaseFixture, database};
use crate::domain::{ReadyState, RunState, TerminalState};
use aidash_application::tenancy::TEST_FIXTURE;
use chrono::{Duration, Utc};
use reinhardt::db::orm::Model;
use rstest::rstest;
use run_fixtures::run;
use uuid::Uuid;

#[rstest]
#[case::ready(RunState::Ready(ReadyState {}),RunControl::Active,false,true)]
#[case::paused(RunState::Ready(ReadyState {}),RunControl::Paused,false,false)]
#[case::completed(RunState::Completed(TerminalState {}),RunControl::Active,false,false)]
#[case::owned(RunState::Ready(ReadyState {}),RunControl::Active,true,false)]
#[tokio::test]
async fn targeted_claim_never_acquires_an_older_neighbor(
	#[future] database: DatabaseFixture,
	run: Record,
	#[case] state: RunState,
	#[case] control: RunControl,
	#[case] owned: bool,
	#[case] eligible: bool,
) {
	// Arrange: the other Run sorts first and has no existing lease.
	let database = database.await;
	let store = Store::from_pool(
		database.connection.into_postgres().unwrap(),
		aidash_domain::nodes::DeploymentId::parse("aidash://targeted-lease-test").unwrap(),
	)
	.await
	.unwrap();
	database.tenant("acme").await;
	let mut db = database.lease.handle();
	let neighbor = Record {
		updated_at: Utc::now() - Duration::minutes(1),
		..run.clone()
	};
	Record::objects()
		.create_with_conn(&mut db, &neighbor)
		.await
		.unwrap();
	let target = Record {
		id: Uuid::new_v4(),
		task_id: Uuid::new_v4(),
		phase: serde_json::from_value(serde_json::json!(state.phase())).unwrap(),
		control,
		pending: run_fixtures::pending(state, Default::default()).into(),
		lease_owner: owned.then(Uuid::new_v4),
		lease_until: owned.then(|| Utc::now() + Duration::minutes(1)),
		..run
	};
	Record::objects()
		.create_with_conn(&mut db, &target)
		.await
		.unwrap();
	let worker = Uuid::new_v4();
	let mut tx = store
		.pool
		.begin(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_executor()
		.await
		.unwrap();
	// Act: notification claims are scoped to exactly one committed Run.
	let claimed = Store::lease_run_in(
		tx.as_mut(),
		worker,
		30,
		Some(target.id),
		store.deployment.as_str(),
		&mut None,
	)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	// Assert: both eligibility and the target predicate retain their scope.
	assert_eq!(claimed.is_some(), eligible);
	if let Some(claimed) = claimed {
		assert_eq!(claimed.id, target.id);
		assert_eq!(claimed.lease_owner, Some(worker));
		assert_eq!(claimed.revision, 1);
	}
	let neighbor = Record::objects()
		.filter(Record::field_id().eq(neighbor.id))
		.get_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(neighbor.phase, RunPhase::Ready);
	assert_eq!(neighbor.revision, 0);
	assert_eq!(neighbor.lease_owner, None);
	let target = Record::objects()
		.filter(Record::field_id().eq(target.id))
		.get_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(target.revision, i64::from(eligible));
}

#[rstest]
#[tokio::test]
async fn leasing_skips_runs_whose_stored_requirement_exceeds_the_worker(
	#[future] database: DatabaseFixture,
	run: Record,
) {
	use crate::domain::run_state::PersistedCapability;
	// Arrange: one Run in the legacy format and one reserved for tool_batch_state workers.
	let database = database.await;
	let store = Store::from_pool(
		database.connection.into_postgres().unwrap(),
		aidash_domain::nodes::DeploymentId::parse("aidash://capability-lease-test").unwrap(),
	)
	.await
	.unwrap();
	database.tenant("acme").await;
	let mut db = database.lease.handle();
	let legacy = Record {
		updated_at: Utc::now() - Duration::minutes(2),
		..run.clone()
	};
	let mut pending = run_fixtures::pending(RunState::Ready(ReadyState {}), Default::default());
	pending["requires"] = serde_json::json!(["tool_batch_state"]);
	let reserved = Record {
		id: Uuid::new_v4(),
		task_id: Uuid::new_v4(),
		updated_at: Utc::now() - Duration::minutes(1),
		pending: pending.into(),
		..run
	};
	for record in [&legacy, &reserved] {
		Record::objects()
			.create_with_conn(&mut db, record)
			.await
			.unwrap();
	}
	let old_worker: &'static [PersistedCapability] = &[];

	// Act: an old worker scans the queue and then tries to claim the reserved Run.
	let mut tx = store
		.pool
		.begin(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_executor()
		.await
		.unwrap();
	let mut scope = crate::bootstrap::activation_scheduling_scope(tx.as_mut());
	scope.capabilities = old_worker;
	let visible: Vec<Uuid> =
		aidash_application::ports::activation::SchedulingScope::candidates(&mut scope, None, None)
			.await
			.unwrap()
			.into_iter()
			.map(|raw| raw.id)
			.collect();
	let claimed = aidash_application::activation::lease(
		&mut scope,
		Uuid::new_v4(),
		30,
		Some(reserved.id),
		store.deployment.as_str(),
		&mut None,
	)
	.await
	.unwrap();
	tx.commit().await.unwrap();

	// Assert: it sees only the legacy Run and leaves the reserved one untouched.
	assert_eq!(visible, [legacy.id]);
	assert!(claimed.is_none());
	let untouched = Record::objects()
		.filter(Record::field_id().eq(reserved.id))
		.get_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(untouched.revision, 0);
	assert_eq!(untouched.lease_owner, None);
	assert_eq!(untouched.phase, RunPhase::Ready);

	// Act: a current worker (production capabilities) claims it right away.
	let mut tx = store
		.pool
		.begin(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_executor()
		.await
		.unwrap();
	let resumed = Store::lease_run_in(
		tx.as_mut(),
		Uuid::new_v4(),
		30,
		Some(reserved.id),
		store.deployment.as_str(),
		&mut None,
	)
	.await
	.unwrap();
	tx.commit().await.unwrap();

	// Assert
	assert_eq!(resumed.unwrap().id, reserved.id);
}

#[rstest]
#[tokio::test]
async fn malformed_requirements_stay_visible_and_hidden_runs_are_counted(
	#[future] database: DatabaseFixture,
	run: Record,
) {
	use crate::domain::run_state::PersistedCapability;
	// Arrange: Runs whose `requires` is malformed, and one a current worker cannot cover.
	let database = database.await;
	let store = Store::from_pool(
		database.connection.into_postgres().unwrap(),
		aidash_domain::nodes::DeploymentId::parse("aidash://capability-malformed-test").unwrap(),
	)
	.await
	.unwrap();
	database.tenant("acme").await;
	let mut db = database.lease.handle();
	let mut ids = vec![];
	for (offset, requires) in [
		serde_json::json!(null),
		serde_json::json!({"tool_batch_state": true}),
		serde_json::json!([1]),
		serde_json::json!(["future_capability"]),
	]
	.into_iter()
	.enumerate()
	{
		let mut pending = run_fixtures::pending(RunState::Ready(ReadyState {}), Default::default());
		pending["requires"] = requires;
		let record = Record {
			id: Uuid::new_v4(),
			task_id: Uuid::new_v4(),
			updated_at: Utc::now() - Duration::minutes(10 - offset as i64),
			pending: pending.into(),
			..run.clone()
		};
		Record::objects()
			.create_with_conn(&mut db, &record)
			.await
			.unwrap();
		ids.push(record.id);
	}
	let current: &'static [PersistedCapability] = PersistedCapability::supported();

	// Act
	let mut tx = store
		.pool
		.begin(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_executor()
		.await
		.unwrap();
	let mut scope = crate::bootstrap::activation_scheduling_scope(tx.as_mut());
	scope.capabilities = current;
	let waiting = scope.capability_waits().await.unwrap();
	let visible: Vec<Uuid> =
		aidash_application::ports::activation::SchedulingScope::candidates(&mut scope, None, None)
			.await
			.unwrap()
			.into_iter()
			.map(|raw| raw.id)
			.collect();
	tx.commit().await.unwrap();

	// Assert: malformed requirements reach decoding (and repair); only the real gap is
	// hidden, and it is counted.
	assert_eq!(visible, ids[..3]);
	assert_eq!(waiting, 1);
}

#[rstest]
#[tokio::test]
async fn a_cancelled_run_no_worker_can_load_is_handed_to_cancellation_delivery(
	#[future] database: DatabaseFixture,
	run: Record,
) {
	use crate::domain::FailureTarget;
	// Arrange: the Deployment Administrator cancelled a Run whose state needs a capability no build has.
	let database = database.await;
	let store = Store::from_pool(
		database.connection.into_postgres().unwrap(),
		aidash_domain::nodes::DeploymentId::parse("aidash://capability-cancel-test").unwrap(),
	)
	.await
	.unwrap();
	database.tenant("acme").await;
	let mut db = database.lease.handle();
	let mut pending = run_fixtures::pending(RunState::Ready(ReadyState {}), Default::default());
	pending["requires"] = serde_json::json!(["future_capability"]);
	let cancelled = Record {
		control: RunControl::Cancelled,
		error: Some("earlier step failure".into()),
		pending: pending.into(),
		..run
	};
	Record::objects()
		.create_with_conn(&mut db, &cancelled)
		.await
		.unwrap();

	// Act: a current worker scans the queue and then claims the Run.
	let mut tx = store
		.pool
		.begin(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_executor()
		.await
		.unwrap();
	let mut scope = crate::bootstrap::activation_scheduling_scope(tx.as_mut());
	let waiting = scope.capability_waits().await.unwrap();
	let visible: Vec<Uuid> =
		aidash_application::ports::activation::SchedulingScope::candidates(&mut scope, None, None)
			.await
			.unwrap()
			.into_iter()
			.map(|raw| raw.id)
			.collect();
	let claimed = aidash_application::activation::lease(
		&mut scope,
		Uuid::new_v4(),
		30,
		Some(cancelled.id),
		store.deployment.as_str(),
		&mut None,
	)
	.await
	.unwrap();
	tx.commit().await.unwrap();

	// Assert: the Run is not hidden or leased; its unloadable state is replaced by
	// cancellation delivery, without an error, and the lease is released.
	assert_eq!(visible, [cancelled.id]);
	assert_eq!(waiting, 0);
	assert!(claimed.is_none());
	let stored = Record::objects()
		.filter(Record::field_id().eq(cancelled.id))
		.get_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(stored.phase, RunPhase::Waiting);
	assert_eq!(stored.pending.0["data"]["reason"], "failure_delivery");
	assert_eq!(stored.pending.0["data"]["target"], "CANCELLED");
	assert!(stored.pending.0.get("requires").is_none());
	assert_eq!(stored.error, None);
	assert_eq!(stored.lease_owner, None);
	assert_eq!(stored.revision, 2);

	// Act: failure delivery, which reads only metadata, picks it up.
	let delivery = store
		.claim_failure_delivery(Uuid::new_v4(), 30)
		.await
		.unwrap()
		.unwrap();

	// Assert
	assert_eq!(delivery.metadata.id, cancelled.id);
	assert_eq!(delivery.target, FailureTarget::Cancelled);
}
