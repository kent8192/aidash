use crate::endpoint::{assert_json, endpoint};
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;

#[rstest]
#[tokio::test]
async fn deployment_administrators_delegate_no_tasks_of_their_own(endpoint: crate::endpoint::EndpointFuture) {
	// Arrange
	let app = endpoint.await;
	let path = format!("/api/tasks/{}/delegate", Uuid::new_v4());
	let input =
		json!({"node_id":"aidash://missing-peer","agent":{"id":"research","version":"1.0.0"}});
	// Act
	let anonymous = app.anonymous.post(&path, &input, "json").await.unwrap();
	let deployment_administrator = app.deployment_administrator.post(&path, &input, "json").await.unwrap();
	// Assert: tasks belong to Tenants, so the Deployment Administrator is refused before any lookup (ADR 0043).
	assert_json(anonymous, 401);
	assert_json(deployment_administrator, 403);
}
