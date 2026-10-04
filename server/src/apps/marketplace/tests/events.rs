use super::*;
use serde_json::json;
use uuid::Uuid;

#[rstest::rstest]
fn recipient_replay_removes_legacy_publisher_subject() {
	let event = Event {
		sequence: 1,
		id: Uuid::nil(),
		node_id: "node".into(),
		workspace_id: None,
		kind: "marketplace.published".into(),
		data: json!({"key":"package","tenant":"owner","actor":"private-subject"}),
		created_at: chrono::Utc::now(),
	};
	let received = recipient_event(event);
	let response = reinhardt::Response::ok()
		.with_json(&received.cloud_event())
		.unwrap();
	let wire: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
	assert_eq!(wire["data"], json!({"key":"package","tenant":"owner"}));
	assert_eq!(wire["sequence"], 1);
}
