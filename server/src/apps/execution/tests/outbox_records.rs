use super::*;
use crate::apps::execution::models::event_records;
use crate::apps::execution::tests::native_database::{DatabaseFixture, database};
use rstest::{fixture, rstest};
use serde_json::json;
use std::{collections::BTreeSet, time::Duration as Timeout};

struct QueuedEvents {
	database: DatabaseFixture,
	events: Vec<Event>,
}

#[fixture]
async fn queued_events(
	#[default(3)] count: i64,
	#[future] database: DatabaseFixture,
) -> QueuedEvents {
	let database = database.await;
	let connection = database.lease.handle();
	let mut events = Vec::new();
	for sequence in 1..=count {
		events.push(
			connection
				.atomic(async |tx| {
					event_records::create(
						tx,
						"aidash://outbox-records",
						None,
						"fixture",
						json!({"sequence":sequence}),
					)
					.await
				})
				.await
				.unwrap(),
		);
	}
	QueuedEvents { database, events }
}

#[rstest]
#[tokio::test]
async fn concurrent_publishers_claim_disjoint_bounded_batches(
	#[future]
	#[with(103)]
	queued_events: QueuedEvents,
) {
	// Arrange
	let fixture = queued_events.await;
	let connection = fixture.database.lease.handle();
	// Act
	let (first, second) = tokio::join!(
		Event::claim_outbox(connection),
		Event::claim_outbox(connection)
	);
	let first = first.unwrap();
	let second = second.unwrap();
	// Assert
	// SKIP LOCKED may interleave row acquisition between the two publishers.
	// Each claim stays bounded, and the two claims cover the queue exactly once.
	assert!((1..=100).contains(&first.len()));
	assert!((1..=100).contains(&second.len()));
	assert_eq!(first.len() + second.len(), fixture.events.len());
	let ids: BTreeSet<_> = first.iter().chain(&second).map(|event| event.id).collect();
	assert_eq!(ids.len(), first.len() + second.len());
	assert_eq!(ids, fixture.events.iter().map(|event| event.id).collect());
	assert!(Event::claim_outbox(connection).await.unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn a_locked_event_does_not_stall_the_next_publisher(#[future] queued_events: QueuedEvents) {
	// Arrange
	let fixture = queued_events.await;
	let connection = fixture.database.lease.handle();
	let locked_id = fixture.events[0].id;
	connection
		.atomic(async |tx| {
			Event::objects()
				.filter(Event::field_id().eq(locked_id))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.unwrap();
			// Act
			let claimed =
				tokio::time::timeout(Timeout::from_secs(3), Event::claim_outbox(connection))
					.await
					.expect("SKIP LOCKED must not wait for this transaction")
					.unwrap();
			// Assert
			assert_eq!(claimed.len(), 2);
			assert!(claimed.iter().all(|event| event.id != locked_id));
			Result::Ok(())
		})
		.await
		.unwrap();
	let remaining = Event::claim_outbox(connection).await.unwrap();
	assert_eq!(
		remaining.iter().map(|event| event.id).collect::<Vec<_>>(),
		[locked_id]
	);
}

#[rstest]
#[tokio::test]
async fn retry_retains_payload_and_fences_late_publication_results(
	#[future]
	#[with(1)]
	queued_events: QueuedEvents,
) {
	// Arrange
	let fixture = queued_events.await;
	let mut connection = fixture.database.lease.handle();
	let first = Event::claim_outbox(connection)
		.await
		.unwrap()
		.pop()
		.unwrap();
	// Act
	assert!(
		first
			.finish_publication(connection, Some("broker unavailable".into()))
			.await
			.unwrap()
	);
	assert!(Event::claim_outbox(connection).await.unwrap().is_empty());
	let saved = Event::objects()
		.filter(Event::field_id().eq(first.id))
		.get_with_db(&mut connection)
		.await
		.unwrap();
	assert_eq!(saved.publish_error.as_deref(), Some("broker unavailable"));
	assert_eq!(saved.data, first.data);
	assert_eq!(saved.published_at, None);
	Event::objects()
		.filter(Event::field_id().eq(first.id))
		.update_fields_with_conn(
			&mut connection,
			[(
				Event::field_next_attempt_at(),
				first.next_attempt_at - Duration::minutes(1),
			)],
		)
		.await
		.unwrap();
	let retry = Event::claim_outbox(connection)
		.await
		.unwrap()
		.pop()
		.unwrap();
	assert!(!first.finish_publication(connection, None).await.unwrap());
	assert!(retry.finish_publication(connection, None).await.unwrap());
	assert!(
		!first
			.finish_publication(connection, Some("stale failure".into()))
			.await
			.unwrap()
	);
	// Assert
	let saved = Event::objects()
		.filter(Event::field_id().eq(first.id))
		.get_with_db(&mut connection)
		.await
		.unwrap();
	assert!(saved.published_at.is_some());
	assert_eq!(saved.publish_error, None);
	assert_eq!(saved.data, first.data);
	assert!(Event::claim_outbox(connection).await.unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn concurrent_duplicate_receipts_have_one_durable_winner(
	#[future] database: DatabaseFixture,
) {
	// Arrange
	let database = database.await;
	let mut connection = database.lease.handle();
	let id = Uuid::new_v4();
	// Act
	let (first, second) = tokio::join!(
		Inbox::receive(connection, id),
		Inbox::receive(connection, id)
	);
	// Assert
	assert_eq!(
		usize::from(first.unwrap()) + usize::from(second.unwrap()),
		1
	);
	assert!(!Inbox::receive(connection, id).await.unwrap());
	let inbox = Inbox::objects()
		.all()
		.all_with_db(&mut connection)
		.await
		.unwrap();
	assert_eq!(inbox.len(), 1);
	assert_eq!(inbox[0].event_id, id);
}
