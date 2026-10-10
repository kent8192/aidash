use crate::endpoint::{assert_json, endpoint, subject};
use rstest::rstest;

#[rstest]
#[tokio::test]
async fn deployment_observations_are_restricted_to_operators(
	endpoint: crate::endpoint::EndpointFuture,
	#[from(crate::endpoint::anonymous_client)]
	#[with(endpoint.clone())]
	_credential_client_0: crate::endpoint::ClientFuture,
) {
	// Arrange
	let app = endpoint.await;
	let alice = subject(&app, "alice", _credential_client_0.await).await;
	// Act
	let anonymous = app.anonymous.get("/api/deployment").await.unwrap();
	let denied = alice.get("/api/deployment").await.unwrap();
	// Assert
	assert_json(anonymous, 401);
	assert_json(denied, 403);
}
