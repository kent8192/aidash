use crate::endpoint::{EndpointFixture, assert_json, endpoint, subject};
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;

#[rstest]
#[tokio::test]
async fn remote_control_checks_operator_and_peer_before_external_io(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let app = endpoint.await;
	let alice = subject(&app, "alice").await;
	let command = json!({"node_id":"aidash://missing-peer","control":{"run_id":Uuid::new_v4(),"action":"pause"}});
	// Act
	let anonymous = app
		.anonymous
		.post("/api/remote", &command, "json")
		.await
		.unwrap();
	let denied = alice.post("/api/remote", &command, "json").await.unwrap();
	let missing_peer = app
		.operator
		.post("/api/remote", &command, "json")
		.await
		.unwrap();
	// Assert
	assert_json(anonymous, 401);
	assert_json(denied, 403);
	assert_json(missing_peer, 401);
}
