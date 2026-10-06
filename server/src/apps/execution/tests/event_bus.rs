//! Durable event delivery through native PostgreSQL, real JetStream and HTTP replay.
#[path = "support/endpoint.rs"]
mod endpoint;
#[path = "support/event_bus.rs"]
mod fixtures;
#[path = "support/native_database.rs"]
mod native_database;

use aidash_server::apps::execution::models::{Event, Inbox};
use async_nats::jetstream::consumer::pull::Config as PullConfig;
use endpoint::assert_json;
use fixtures::{BusFixture, event_bus};
use reinhardt::db::orm::Model;
use rstest::rstest;
use serde_json::{Value, json};
use std::time::Duration;

#[rstest]
#[tokio::test]
async fn oversized_payload_uses_an_http_replay_reference_without_blocking_later_events(
	#[future] event_bus: BusFixture,
) {
	// Arrange
	let fixture = event_bus.await;
	let store = &fixture.app.runtime.store;
	let large = store
		.emit(None, "large", json!({"text":"x".repeat(2_000_000)}))
		.await
		.unwrap();
	let small = store.emit(None, "small", json!({"ok":true})).await.unwrap();
	// Act
	assert_eq!(
		fixture
			.bus
			.publish_once(&fixture.app.runtime)
			.await
			.unwrap(),
		2
	);
	// Assert
	let stream = fixture
		.bus
		.context
		.get_stream(&fixture.bus.stream_name)
		.await
		.unwrap();
	let mut reference = None;
	for sequence in 1..=2 {
		let message = stream.get_raw_message(sequence).await.unwrap();
		let envelope: Value = serde_json::from_slice(&message.payload).unwrap();
		if envelope["id"] == large.id.to_string() {
			assert!(envelope.get("data").is_none());
			assert_eq!(
				envelope["dataref"],
				format!("/api/events?after={}", large.sequence - 1)
			);
			reference = Some(envelope["dataref"].as_str().unwrap().to_owned());
		} else {
			assert_eq!(envelope["id"], small.id.to_string());
			assert_eq!(envelope["data"], small.data);
		}
	}
	let replay = assert_json(
		fixture
			.app
			.operator
			.get(&reference.expect("oversized event has a reference"))
			.await
			.unwrap(),
		200,
	);
	assert_eq!(
		replay
			.as_array()
			.unwrap()
			.iter()
			.find(|event| event["id"] == large.id.to_string())
			.unwrap()["data"],
		large.data
	);
	let records = Event::objects()
		.all()
		.all_with_db(&mut fixture.app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(records.len(), 2);
	assert!(
		records
			.iter()
			.all(|event| event.published_at.is_some() && event.publish_error.is_none())
	);
	assert_eq!(
		fixture
			.bus
			.publish_once(&fixture.app.runtime)
			.await
			.unwrap(),
		0
	);
}

#[rstest]
#[tokio::test]
async fn consumer_commits_one_receipt_per_event_and_acknowledges_duplicates_and_bad_envelopes(
	#[future] event_bus: BusFixture,
) {
	// Arrange
	let fixture = event_bus.await;
	let event = fixture
		.app
		.runtime
		.store
		.emit(None, "wake", json!({"step":1}))
		.await
		.unwrap();
	let wake = fixture.app.runtime.notify.notified();
	tokio::pin!(wake);
	wake.as_mut().enable();
	let consumer = fixture.consumer();
	// Act
	assert_eq!(
		fixture
			.bus
			.publish_once(&fixture.app.runtime)
			.await
			.unwrap(),
		1
	);
	tokio::time::timeout(Duration::from_secs(10), &mut wake)
		.await
		.expect("durable receipt wakes workers");
	for payload in [
		event.cloud_event().to_string(),
		event.cloud_event().to_string(),
		"not JSON".into(),
	] {
		fixture
			.bus
			.context
			.publish(fixture.bus.subject.clone(), payload.into())
			.await
			.unwrap()
			.await
			.unwrap();
	}
	let stream = fixture
		.bus
		.context
		.get_stream(&fixture.bus.stream_name)
		.await
		.unwrap();
	let mut execution = stream
		.get_consumer::<PullConfig>("execution")
		.await
		.unwrap();
	tokio::time::timeout(Duration::from_secs(10), async {
		loop {
			let info = execution.info().await.unwrap();
			if info.ack_floor.stream_sequence == 4
				&& info.num_ack_pending == 0
				&& info.num_pending == 0
			{
				break;
			}
			assert!(
				!consumer.is_finished(),
				"consumer exits on a malformed or duplicate message"
			);
			tokio::time::sleep(Duration::from_millis(25)).await;
		}
	})
	.await
	.expect("all duplicate and malformed messages are acknowledged");
	// Assert
	let inbox = Inbox::objects()
		.all()
		.all_with_db(&mut fixture.app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(inbox.len(), 1);
	assert_eq!(inbox[0].event_id, event.id);
	consumer.stop().await;
}
