use crate::native_database::{DatabaseFixture, database};
use aidash_server::Error;
use aidash_server::apps::federation::transactions::models::{
	AtomicCoordinator, AtomicHistory, coordinator_records, states::AtomicCoordinatorDecision,
};
use aidash_server::apps::federation::transactions::services::decisions::CoordinatorTransition;
use reinhardt::db::orm::Model;
use rstest::{fixture, rstest};
use serde_json::json;

#[fixture]
fn coordinator() -> AtomicCoordinator {
	AtomicCoordinator::build()
		.digest("immutable-manifest")
		.manifest(json!({"coordinator": "aidash://test", "participants": []}).into())
		.decision(None)
		.visible(false)
		.complete(false)
		.last_error(None)
		.finish()
}

#[rstest]
#[tokio::test]
async fn racing_decisions_are_immutable_and_audited_once(
	#[future] database: DatabaseFixture,
	coordinator: AtomicCoordinator,
) {
	let database = database.await;
	let mut connection = database.lease.handle();
	let original = AtomicCoordinator::objects()
		.create_with_conn(&mut connection, &coordinator)
		.await
		.unwrap();
	let (commit, abort) = tokio::join!(
		coordinator_records::transition(
			connection,
			original.id,
			CoordinatorTransition::Decide(AtomicCoordinatorDecision::Commit),
			""
		),
		coordinator_records::transition(
			connection,
			original.id,
			CoordinatorTransition::Decide(AtomicCoordinatorDecision::Abort),
			"cancel"
		),
	);
	assert_ne!(commit.unwrap(), abort.unwrap());
	let selected = AtomicCoordinator::objects()
		.filter(AtomicCoordinator::field_id().eq(original.id))
		.get_with_db(&mut connection)
		.await
		.unwrap();
	let opposite = if selected.decision == Some(AtomicCoordinatorDecision::Commit) {
		AtomicCoordinatorDecision::Abort
	} else {
		AtomicCoordinatorDecision::Commit
	};
	assert!(
		!coordinator_records::transition(
			connection,
			original.id,
			CoordinatorTransition::Decide(opposite),
			"retry"
		)
		.await
		.unwrap()
	);
	let preserved = AtomicCoordinator::objects()
		.filter(AtomicCoordinator::field_id().eq(original.id))
		.get_with_db(&mut connection)
		.await
		.unwrap();
	assert_eq!(preserved.id, original.id);
	assert_eq!(preserved.digest, original.digest);
	assert_eq!(preserved.manifest, original.manifest);
	assert_eq!(preserved.decision, selected.decision);
	assert_eq!(preserved.last_error, selected.last_error);
	let history = AtomicHistory::objects()
		.filter(AtomicHistory::field_transaction_id().eq(original.id))
		.all_with_db(&mut connection)
		.await
		.unwrap();
	assert_eq!(history.len(), 1);
}

#[rstest]
#[case(AtomicCoordinatorDecision::Commit)]
#[case(AtomicCoordinatorDecision::Abort)]
#[tokio::test]
async fn visibility_and_completion_are_monotonic(
	#[future] database: DatabaseFixture,
	coordinator: AtomicCoordinator,
	#[case] decision: AtomicCoordinatorDecision,
) {
	let database = database.await;
	let mut connection = database.lease.handle();
	let original = AtomicCoordinator::objects()
		.create_with_conn(&mut connection, &coordinator)
		.await
		.unwrap();
	let premature = coordinator_records::transition(
		connection,
		original.id,
		CoordinatorTransition::Complete,
		"",
	)
	.await;
	assert!(matches!(premature, Err(Error::Conflict(_))));
	coordinator_records::transition(
		connection,
		original.id,
		CoordinatorTransition::Decide(decision.clone()),
		"",
	)
	.await
	.unwrap();
	if decision == AtomicCoordinatorDecision::Commit {
		let premature = coordinator_records::transition(
			connection,
			original.id,
			CoordinatorTransition::Complete,
			"",
		)
		.await;
		assert!(matches!(premature, Err(Error::Conflict(_))));
		assert!(
			coordinator_records::transition(
				connection,
				original.id,
				CoordinatorTransition::Publish,
				"applied"
			)
			.await
			.unwrap()
		);
		assert!(
			!coordinator_records::transition(
				connection,
				original.id,
				CoordinatorTransition::Publish,
				"retry"
			)
			.await
			.unwrap()
		);
	} else {
		let publish = coordinator_records::transition(
			connection,
			original.id,
			CoordinatorTransition::Publish,
			"",
		)
		.await;
		assert!(matches!(publish, Err(Error::Conflict(_))));
	}
	assert!(
		coordinator_records::transition(
			connection,
			original.id,
			CoordinatorTransition::Complete,
			"finalized"
		)
		.await
		.unwrap()
	);
	assert!(
		!coordinator_records::transition(
			connection,
			original.id,
			CoordinatorTransition::Complete,
			"retry"
		)
		.await
		.unwrap()
	);
	let final_state = AtomicCoordinator::objects()
		.filter(AtomicCoordinator::field_id().eq(original.id))
		.get_with_db(&mut connection)
		.await
		.unwrap();
	assert!(final_state.complete);
	assert_eq!(
		final_state.visible,
		decision == AtomicCoordinatorDecision::Commit
	);
	assert_eq!(final_state.decision, Some(decision));
	assert_eq!(final_state.digest, original.digest);
	assert_eq!(final_state.manifest, original.manifest);
}

#[rstest]
#[tokio::test]
async fn audit_detail_preserves_quotes_and_placeholder_text(
	#[future] database: DatabaseFixture,
	coordinator: AtomicCoordinator,
) {
	let database = database.await;
	let mut connection = database.lease.handle();
	let original = AtomicCoordinator::objects()
		.create_with_conn(&mut connection, &coordinator)
		.await
		.unwrap();
	let detail = "provider said '$1 $2 $3 $4' literally";
	coordinator_records::transition(
		connection,
		original.id,
		CoordinatorTransition::Decide(AtomicCoordinatorDecision::Abort),
		detail,
	)
	.await
	.unwrap();
	let history = AtomicHistory::objects()
		.filter(AtomicHistory::field_transaction_id().eq(original.id))
		.all_with_db(&mut connection)
		.await
		.unwrap();
	assert_eq!(history.len(), 1);
	assert_eq!(history[0].detail, detail);
	let state = AtomicCoordinator::objects()
		.filter(AtomicCoordinator::field_id().eq(original.id))
		.get_with_db(&mut connection)
		.await
		.unwrap();
	assert_eq!(state.last_error.as_deref(), Some(detail));
}
