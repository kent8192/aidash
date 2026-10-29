//! Durable event delivery through native PostgreSQL, real JetStream and HTTP replay.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "support/endpoint.rs"]
mod endpoint;
#[path = "support/event_bus.rs"]
mod fixtures;
#[path = "support/native_database.rs"]
mod native_database;

use aidash_application::tenancy::TenancyScope;
use aidash_server::apps::execution::models::{Event, Inbox};
use async_nats::jetstream::consumer::pull::Config as PullConfig;
use endpoint::assert_json;
use fixtures::{BusFixture, event_bus};
use reinhardt::db::orm::Model;
use rstest::rstest;
use serde_json::{Value, json};
use std::time::Duration;

/// The scope of a Tenant whose root record exists, since every event names its Tenant.
async fn acme(fixture: &BusFixture) -> TenancyScope {
	endpoint::tenant_root_in(fixture.app.runtime.store.pool.fixture_driver(), "acme").await;
	TenancyScope::tenant("acme").unwrap()
}

#[rstest]
#[tokio::test]
async fn oversized_payload_uses_an_http_replay_reference_without_blocking_later_events(
	#[future] event_bus: BusFixture,
) {
	// Arrange
	let fixture = event_bus.await;
	let store = &fixture.app.runtime.store;
	let scope = acme(&fixture).await;
	// A Subject replays only the events of Workspaces it may read (#484), so
	// the events are Workspace events of the Tenant. Creating the Workspace
	// emits a first event of its own.
	let (member, _) = endpoint::tenant_member(&fixture.app).await;
	let selected = endpoint::workspace(&member, "Event bus replay").await;
	let workspace = uuid::Uuid::parse_str(selected["id"].as_str().unwrap()).unwrap();
	let large = store
		.emit(
			&scope,
			Some(workspace),
			"workspace.updated",
			json!({"text":"x".repeat(2_000_000)}),
		)
		.await
		.unwrap();
	let small = store
		.emit(
			&scope,
			Some(workspace),
			"workspace.updated",
			json!({"ok":true}),
		)
		.await
		.unwrap();
	// Act
	assert_eq!(
		fixture
			.bus
			.publish_once(&fixture.app.runtime)
			.await
			.unwrap(),
		3
	);
	// Assert
	let stream = fixture
		.bus
		.context
		.get_stream(&fixture.bus.stream_name)
		.await
		.unwrap();
	let mut reference = None;
	for sequence in 1..=3 {
		let message = stream.get_raw_message(sequence).await.unwrap();
		let envelope: Value = serde_json::from_slice(&message.payload).unwrap();
		if envelope["id"] == large.id.to_string() {
			assert!(envelope.get("data").is_none());
			assert_eq!(
				envelope["dataref"],
				format!("/api/events?after={}", large.sequence - 1)
			);
			reference = Some(envelope["dataref"].as_str().unwrap().to_owned());
		} else if envelope["id"] == small.id.to_string() {
			assert_eq!(envelope["data"], small.data);
		} else {
			// The Workspace's own event, published before the two above.
			assert!(envelope["id"].is_string());
		}
	}
	// The reference replays only to a Tenant Subject; a Deployment Administrator
	// holds no Tenant content authority (#484).
	let reference = reference.expect("oversized event has a reference");
	let refused = fixture
		.app
		.deployment_administrator
		.get(&reference)
		.await
		.unwrap();
	assert_eq!(refused.status_code(), 403, "{}", refused.text());
	assert!(!refused.text().contains("xxxxxxxx"));
	let replay = assert_json(member.get(&reference).await.unwrap(), 200);
	assert_eq!(
		replay
			.as_array()
			.unwrap()
			.iter()
			.find(|event| event["id"] == large.id.to_string())
			.unwrap_or_else(|| panic!(
				"{}",
				replay.to_string().chars().take(600).collect::<String>()
			))["data"],
		large.data
	);
	let records = Event::objects()
		.all()
		.all_with_db(&mut fixture.app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(records.len(), 3);
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
	let scope = acme(&fixture).await;
	let event = fixture
		.app
		.runtime
		.store
		.emit(&scope, None, "wake", json!({"step":1}))
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
		event.cloud_event("aidash://fixture").to_string(),
		event.cloud_event("aidash://fixture").to_string(),
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
