use crate::endpoint::{assert_json, endpoint};
use aidash_server::config::PROTOCOL_VERSION;
use rstest::{fixture, rstest};
use serde_json::json;

#[rstest]
#[tokio::test]
async fn identity_is_public_but_federation_requires_a_peer_credential(
	endpoint: crate::endpoint::EndpointFuture,
	#[from(untrusted_client)]
	#[with(endpoint.clone())]
	untrusted: crate::endpoint::ClientFuture,
) {
	// Arrange
	let app = endpoint.await;
	let untrusted = untrusted.await;
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

#[fixture]
fn untrusted_client(
	#[from(endpoint)] _endpoint: crate::endpoint::EndpointFuture,
	#[from(crate::endpoint::anonymous_client)]
	#[with(_endpoint.clone())]
	client: crate::endpoint::ClientFuture,
) -> crate::endpoint::ClientFuture {
	use futures_util::FutureExt;
	async move {
		let client = client.await;
		for (name, value) in [
			("x-aidash-protocol", PROTOCOL_VERSION),
			("x-aidash-node", "aidash://unknown-peer"),
			("Authorization", "Bearer invalid-peer-credential"),
		] {
			client.set_header(name, value).await.unwrap();
		}
		client
	}
	.boxed()
	.shared()
}
