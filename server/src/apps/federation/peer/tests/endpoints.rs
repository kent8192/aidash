use crate::endpoint::{EndpointFixture, assert_json, endpoint};
use aidash_server::config::PROTOCOL_VERSION;
use reinhardt::test::fixtures::api_client_from_url;
use rstest::rstest;
use serde_json::json;

#[rstest]
#[tokio::test]
async fn identity_is_public_but_federation_requires_a_peer_credential(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let app = endpoint.await;
	let untrusted = api_client_from_url(&app.server.url);
	untrusted
		.set_header("x-aidash-protocol", PROTOCOL_VERSION)
		.await
		.unwrap();
	untrusted
		.set_header("x-aidash-node", "aidash://unknown-peer")
		.await
		.unwrap();
	untrusted
		.set_header("Authorization", "Bearer invalid-peer-credential")
		.await
		.unwrap();
	// Act
	let identity = app.anonymous.get("/.well-known/aidash").await.unwrap();
	let missing_protocol = app
		.anonymous
		.post("/federation/v0.1/discover", &json!({}), "json")
		.await
		.unwrap();
	let invalid_peer = untrusted
		.post("/federation/v0.1/discover", &json!({}), "json")
		.await
		.unwrap();
	// Assert
	let identity = assert_json(identity, 200);
	assert_eq!(identity["id"], app.runtime.config.node_id);
	assert_eq!(identity["protocol_version"], PROTOCOL_VERSION);
	assert_json(missing_protocol, 400);
	assert_json(invalid_peer, 401);
}
