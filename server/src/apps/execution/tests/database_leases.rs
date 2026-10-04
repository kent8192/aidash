use crate::native_database::{DatabaseFixture, database};
use crate::run_fixtures::{human_pending, pending as encode_pending, run};
use aidash_server::{
	apps::execution::models::{
		HumanRequest, Run,
		states::{HumanRequestKind, RunControl, RunPhase},
	},
	store::Store,
};
use chrono::{Duration, Utc};
use reinhardt::db::orm::Model;
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;

#[rstest]
#[tokio::test]
async fn concurrent_workers_skip_locked_rows_and_recover_expired_leases(
	#[future] database: DatabaseFixture,
	run: Run,
) {
	let database = database.await;
	let store = Store::from_pool(
		database.connection.into_postgres().unwrap(),
		"aidash://lease-test".into(),
	)
	.await
	.unwrap();
	let mut db = database.lease.handle();
	let now = Utc::now();
	let mut records = Vec::new();
	for index in 0..3 {
		let mut record = run.clone();
		record.id = Uuid::new_v4();
		record.task_id = Uuid::new_v4();
		record.updated_at = now + Duration::seconds(index);
		if index == 1 {
			record.lease_owner = Some(Uuid::new_v4());
			record.lease_until = Some(now - Duration::seconds(1));
		}
		records.push(
			Run::objects()
				.create_with_conn(&mut db, &record)
				.await
				.unwrap(),
		);
	}
	let mut lock = database.connection.begin().await.unwrap();
	Run::objects()
		.filter(Run::field_id().eq(records[0].id))
		.select_for_update()
		.all_with_executor(lock.as_mut())
		.await
		.unwrap();
	let a = Uuid::new_v4();
	let b = Uuid::new_v4();
	let (first, second) = tokio::join!(store.lease_run(a, 30), store.lease_run(b, 30));
	let first = first.unwrap().unwrap();
	let second = second.unwrap().unwrap();
	assert_ne!(first.id, second.id);
	assert_eq!(first.lease_owner, Some(a));
	assert_eq!(second.lease_owner, Some(b));
	for leased in [first, second] {
		assert_ne!(leased.id, records[0].id);
		assert_eq!(leased.revision, 1);
		assert!(leased.lease_until.unwrap() > now);
		assert_eq!(leased.recovery.lease_recovered, leased.id == records[1].id);
	}
	assert!(store.lease_run(Uuid::new_v4(), 30).await.unwrap().is_none());
	lock.rollback().await.unwrap();
	let previously_locked = store.lease_run(a, 30).await.unwrap().unwrap();
	assert_eq!(previously_locked.id, records[0].id);
	assert_eq!(previously_locked.revision, 1);
	let saved = Run::objects()
		.filter(Run::field_id().eq(records[0].id))
		.get_with_db(&mut db)
		.await
		.unwrap();
	assert!(saved.ledger_worker_ready);
	assert_eq!(saved.lease_owner, Some(a));
}

#[rstest]
#[tokio::test]
async fn workers_filter_terminal_paused_retry_and_waiting_records(
	#[future] database: DatabaseFixture,
	run: Run,
) {
	let database = database.await;
	let store = Store::from_pool(
		database.connection.into_postgres().unwrap(),
		"aidash://lease-test".into(),
	)
	.await
	.unwrap();
	let mut db = database.lease.handle();
	use aidash_server::domain::{
		ReadyState, RecoveryState, ResumeState, RetryState, RunState, TerminalState, WaitingState,
	};
	let now = Utc::now();
	let ready = RunState::Ready(ReadyState {});
	let retry = |at| RecoveryState {
		retry: Some(RetryState { count: 1, at }),
		..Default::default()
	};
	let timer = |wake_at| {
		RunState::Waiting(Box::new(WaitingState::Timer {
			wake_at,
			resume: ResumeState::Ready(ReadyState {}),
		}))
	};
	let mut eligible = Vec::new();
	for (index, (state, control, recovery, expected)) in [
		(
			RunState::Completed(TerminalState {}),
			RunControl::Active,
			RecoveryState::default(),
			false,
		),
		(
			RunState::Failed(TerminalState {}),
			RunControl::Active,
			RecoveryState::default(),
			false,
		),
		(
			RunState::Cancelled(TerminalState {}),
			RunControl::Active,
			RecoveryState::default(),
			false,
		),
		(
			ready.clone(),
			RunControl::Paused,
			RecoveryState::default(),
			false,
		),
		(
			ready.clone(),
			RunControl::Active,
			retry(now + Duration::hours(1)),
			false,
		),
		(
			timer(now + Duration::hours(1)),
			RunControl::Active,
			RecoveryState::default(),
			false,
		),
		(
			ready,
			RunControl::Active,
			retry(now - Duration::hours(1)),
			true,
		),
		(
			timer(now - Duration::hours(1)),
			RunControl::Active,
			RecoveryState::default(),
			true,
		),
		(
			timer(now + Duration::hours(1)),
			RunControl::Cancelled,
			RecoveryState::default(),
			true,
		),
	]
	.into_iter()
	.enumerate()
	{
		let record = Run {
			id: Uuid::new_v4(),
			task_id: Uuid::new_v4(),
			phase: serde_json::from_value(json!(state.phase())).unwrap(),
			control,
			pending: encode_pending(state, recovery).into(),
			updated_at: Utc::now() + Duration::seconds(index as i64),
			..run.clone()
		};
		let saved = Run::objects()
			.create_with_conn(&mut db, &record)
			.await
			.unwrap();
		if expected {
			eligible.push((saved.updated_at, saved.id));
		}
	}
	// A waiting request becomes runnable only after its persisted response exists.
	for answered in [false, true] {
		let request_id = Uuid::new_v4();
		let record = Run {
			id: Uuid::new_v4(),
			task_id: Uuid::new_v4(),
			phase: RunPhase::Ready,
			updated_at: Utc::now() + Duration::seconds(20),
			..run.clone()
		};
		let mut saved = Run::objects()
			.create_with_conn(&mut db, &record)
			.await
			.unwrap();
		let request = HumanRequest::build()
			.id(request_id)
			.workspace_id(saved.workspace_id)
			.run_id(saved.id)
			.kind(HumanRequestKind::Question)
			.prompt("Continue?")
			.response(answered.then(|| json!({"answer":"yes"}).into()))
			.request_key(format!("worker-lease-fixture-{request_id}"))
			.answered_by(None)
			.finish();
		HumanRequest::objects()
			.create_with_conn(&mut db, &request)
			.await
			.unwrap();
		saved.phase = RunPhase::Waiting;
		saved.pending = human_pending(request_id).into();
		let saved = Run::objects()
			.update_with_conn(&mut db, &saved)
			.await
			.unwrap();
		if answered {
			eligible.push((saved.updated_at, saved.id));
		}
	}
	// Updates refresh timestamps; queue order is the persisted timestamp and ID.
	let persisted = Run::objects().all().all_with_db(&mut db).await.unwrap();
	for (updated_at, id) in &mut eligible {
		*updated_at = persisted
			.iter()
			.find(|run| run.id == *id)
			.unwrap()
			.updated_at;
	}
	eligible.sort();
	for (_, id) in eligible {
		let leased = store.lease_run(Uuid::new_v4(), 30).await.unwrap().unwrap();
		assert_eq!(
			leased.id, id,
			"unexpected queue record: state={:?}, recovery={:?}",
			leased.state, leased.recovery
		);
	}
	assert!(store.lease_run(Uuid::new_v4(), 30).await.unwrap().is_none());
}
