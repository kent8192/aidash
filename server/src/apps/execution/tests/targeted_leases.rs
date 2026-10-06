#[path = "run_fixtures.rs"]
mod run_fixtures;
use super::Store;
use crate::apps::execution::models::{
	Run as Record,
	states::{RunControl, RunPhase},
};
use crate::apps::execution::tests::native_database::{DatabaseFixture, database};
use crate::domain::{ReadyState, RunState, TerminalState};
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
		"aidash://targeted-lease-test".into(),
	)
	.await
	.unwrap();
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
	let mut tx = store.database().begin().await.unwrap();
	// Act: notification claims are scoped to exactly one committed Run.
	let claimed = Store::lease_run_in(
		tx.as_mut(),
		worker,
		30,
		Some(target.id),
		&store.node_id,
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
