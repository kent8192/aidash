use crate::endpoint::{EndpointFixture, assert_json, endpoint, subject};
use rstest::rstest;

#[rstest]
#[tokio::test]
async fn deployment_observations_are_restricted_to_operators(#[future] endpoint: EndpointFixture) {
	// Arrange
	let app = endpoint.await;
	let alice = subject(&app, "alice").await;
	// Act
	let anonymous = app.anonymous.get("/api/deployment").await.unwrap();
	let denied = alice.get("/api/deployment").await.unwrap();
	// Assert
	assert_json(anonymous, 401);
	assert_json(denied, 403);
}
