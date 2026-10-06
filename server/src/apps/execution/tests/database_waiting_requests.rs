use crate::native_database::{DatabaseFixture, database};
use crate::run_fixtures::{human_pending, pending, run};
use aidash_server::apps::execution::models::{
	HumanRequest, Run,
	states::{HumanRequestKind, RunPhase},
};
use reinhardt::db::orm::Model;
use rstest::rstest;
use uuid::Uuid;

#[rstest]
#[tokio::test]
async fn waiting_requests_enforce_identity_ownership_and_reverse_references(
	#[future] database: DatabaseFixture,
	run: Run,
) {
	// Arrange: two independent runs each own one request.
	let database = database.await;
	let mut db = database.lease.handle();
	let mut waiting = Run::objects()
		.create_with_conn(&mut db, &run)
		.await
		.unwrap();
	let other = Run::objects()
		.create_with_conn(
			&mut db,
			&Run {
				id: Uuid::new_v4(),
				task_id: Uuid::new_v4(),
				..run.clone()
			},
		)
		.await
		.unwrap();
	let mut requests = Vec::new();
	for owner in [&waiting, &other] {
		let request = HumanRequest::build()
			.id(Uuid::new_v4())
			.workspace_id(owner.workspace_id)
			.run_id(owner.id)
			.kind(HumanRequestKind::Question)
			.prompt("Continue?")
			.response(None)
			.request_key(format!("waiting-relation-{}", owner.id))
			.answered_by(None)
			.finish();
		requests.push(
			HumanRequest::objects()
				.create_with_conn(&mut db, &request)
				.await
				.unwrap(),
		);
	}

	// Act and assert: writing JSON alone must enforce existence and ownership.
	waiting.phase = RunPhase::Waiting;
	for invalid in [Uuid::new_v4(), requests[1].id] {
		waiting.pending = human_pending(invalid).into();
		let rejected = Run::objects().update_with_conn(&mut db, &waiting).await;
		assert!(rejected.is_err(), "invalid waiting reference was saved");
		let unchanged = Run::objects()
			.get(waiting.id)
			.get_with_db(&mut db)
			.await
			.unwrap();
		assert_eq!(unchanged.phase, RunPhase::Ready);
		assert_eq!(unchanged.pending.0, run.pending.0);
		assert_eq!(unchanged.pending_human_request_id, None);
	}
	waiting.pending = human_pending(requests[0].id).into();
	// Callers cannot override the projected identity by supplying another scalar.
	waiting.pending_human_request_id = Some(requests[1].id);
	Run::objects()
		.update_with_conn(&mut db, &waiting)
		.await
		.unwrap();
	let saved = Run::objects()
		.get(waiting.id)
		.get_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(saved.pending_human_request_id, Some(requests[0].id));
	assert!(
		HumanRequest::objects()
			.delete_with_conn(&mut db, requests[0].id)
			.await
			.is_err()
	);
	assert!(
		HumanRequest::objects()
			.get(requests[0].id)
			.get_with_db(&mut db)
			.await
			.is_ok()
	);

	// Clearing the pending state releases the FK without manual column updates.
	waiting.phase = RunPhase::Ready;
	waiting.pending = pending(Default::default(), Default::default()).into();
	Run::objects()
		.update_with_conn(&mut db, &waiting)
		.await
		.unwrap();
	let resumed = Run::objects()
		.get(waiting.id)
		.get_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(resumed.pending_human_request_id, None);
	HumanRequest::objects()
		.delete_with_conn(&mut db, requests[0].id)
		.await
		.unwrap();
}
