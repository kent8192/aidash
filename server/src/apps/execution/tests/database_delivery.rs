use crate::native_database::{DatabaseFixture, database};
use crate::run_fixtures::run;
use aidash_server::{
	apps::execution::models::{Run, RunInput, states::RunPhase},
	store::Store,
};
use chrono::{Duration, Utc};
use reinhardt::db::orm::Model;
use rstest::{fixture, rstest};
use uuid::Uuid;

#[fixture]
fn terminal_run(mut run: Run) -> Run {
	run.phase = RunPhase::Failed;
	run.error = Some("fixture failure".into());
	run
}

#[rstest]
#[tokio::test]
async fn terminal_delivery_selects_only_due_remote_inputs(
	#[future] database: DatabaseFixture,
	terminal_run: Run,
) {
	let database = database.await;
	let pool = database.connection.into_postgres().unwrap();
	let store = Store::from_pool(pool, "aidash://local-delivery".into())
		.await
		.unwrap();
	let mut db = database.lease.handle();
	let mut records = Vec::new();
	// Arrange earlier ineligible rows before two eligible terminal deliveries.
	for (index, (home, phase, delivered, retry, input)) in [
		(
			"aidash://local-delivery",
			RunPhase::Completed,
			false,
			None,
			true,
		),
		(
			"aidash://remote-delivery",
			RunPhase::Ready,
			false,
			None,
			true,
		),
		(
			"aidash://remote-delivery",
			RunPhase::Completed,
			true,
			None,
			true,
		),
		(
			"aidash://remote-delivery",
			RunPhase::Failed,
			false,
			Some(Utc::now() + Duration::hours(1)),
			true,
		),
		(
			"aidash://remote-delivery",
			RunPhase::Cancelled,
			false,
			None,
			false,
		),
		(
			"aidash://remote-delivery",
			RunPhase::Failed,
			false,
			None,
			true,
		),
		(
			"aidash://remote-delivery",
			RunPhase::Cancelled,
			false,
			Some(Utc::now() - Duration::hours(1)),
			true,
		),
	]
	.into_iter()
	.enumerate()
	{
		let run = Run {
			id: Uuid::new_v4(),
			task_id: Uuid::new_v4(),
			home_node: home.into(),
			phase,
			updated_at: Utc::now() + Duration::seconds(index as i64),
			..terminal_run.clone()
		};
		Run::objects()
			.create_with_conn(&mut db, &run)
			.await
			.unwrap();
		if input {
			let record = RunInput::build()
				.seq(index as i64 + 1)
				.run_id(run.id)
				.sender("human")
				.content("retain this input after terminal completion")
				.message_id(delivered.then(Uuid::new_v4))
				.idempotency_key(format!("input-{index}"))
				.delivery_retry_at(retry)
				.reference_only(false)
				.finish();
			RunInput::objects()
				.create_with_conn(&mut db, &record)
				.await
				.unwrap();
		}
		records.push(run);
	}
	// Act / Assert: LIMIT is applied after all delivery predicates, and enum and
	// metadata round-trips into the worker's delivery contract.
	let first = store.pending_terminal_run_message().await.unwrap().unwrap();
	assert_eq!(first.id, records[5].id);
	assert_eq!(first.phase, aidash_server::domain::RunPhase::Failed);
	assert_eq!(first.control, aidash_server::domain::RunControl::Active);
	assert_eq!(first.error.as_deref(), Some("fixture failure"));
	let mut delivered = RunInput::objects()
		.filter(RunInput::field_run_id().eq(first.id))
		.get_with_db(&mut db)
		.await
		.unwrap();
	delivered.message_id = Some(Uuid::new_v4());
	RunInput::objects()
		.update_with_conn(&mut db, &delivered)
		.await
		.unwrap();
	let next = store.pending_terminal_run_message().await.unwrap().unwrap();
	assert_eq!(next.id, records[6].id);
	assert_eq!(next.phase, aidash_server::domain::RunPhase::Cancelled);

	// A retry postpones only undelivered inputs belonging to the selected run.
	let before = Utc::now();
	store.defer_run_message_delivery(next.id).await.unwrap();
	assert!(
		store
			.pending_terminal_run_message()
			.await
			.unwrap()
			.is_none()
	);
	let postponed = RunInput::objects()
		.filter(RunInput::field_run_id().eq(next.id))
		.get_with_db(&mut db)
		.await
		.unwrap();
	let deadline = postponed.delivery_retry_at.unwrap();
	assert!(deadline >= before + Duration::seconds(4));
	assert!(deadline <= Utc::now() + Duration::seconds(6));
	store.defer_run_message_delivery(first.id).await.unwrap();
	let already_delivered = RunInput::objects()
		.filter(RunInput::field_run_id().eq(first.id))
		.get_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(already_delivered.message_id, delivered.message_id);
	assert_eq!(already_delivered.delivery_retry_at, None);
}
