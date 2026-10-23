use crate::endpoint::{EndpointFuture, assert_json, endpoint};
use aidash_application::federation::key::DeploymentKey;
use aidash_integrations::federation::key::FileDeploymentKey;
use rstest::rstest;
use std::{collections::BTreeSet, sync::Arc};

fn fields(identity: &serde_json::Value) -> BTreeSet<&str> {
	identity
		.as_object()
		.unwrap()
		.keys()
		.map(String::as_str)
		.collect()
}

#[rstest]
#[tokio::test]
async fn a_deployment_without_a_key_publishes_its_identity_and_a_null_key(
	endpoint: EndpointFuture,
) {
	// Arrange
	let app = endpoint.await;
	// Act
	let identity = assert_json(app.anonymous.get("/.well-known/aidash").await.unwrap(), 200);
	// Assert
	assert_eq!(identity["id"], app.runtime.config.deployment);
	assert_eq!(identity["endpoint"], app.runtime.config.endpoint);
	assert_eq!(
		identity["protocol_version"],
		aidash_server::config::PROTOCOL_VERSION
	);
	assert!(identity["key"].is_null());
	assert_eq!(
		fields(&identity),
		BTreeSet::from(["endpoint", "id", "key", "protocol_version"]),
		"the document names no Node, capability or cluster"
	);
}

#[rstest]
#[tokio::test]
async fn a_deployment_with_a_key_file_publishes_the_key_id_and_the_public_key(
	endpoint: EndpointFuture,
) {
	// Arrange: the key the server's settings name, loaded the way startup loads it.
	let app = endpoint.await;
	let directory = tempfile::tempdir().unwrap();
	let path = directory.path().join("deployment-key.pem");
	FileDeploymentKey::generate(&path, false).unwrap();
	let key = FileDeploymentKey::load(&path).unwrap();
	let (id, pem) = (key.key_id().to_string(), key.public_key_pem().to_owned());
	let mut runtime = app.runtime.clone();
	runtime.config.deployment_key = Some(Arc::new(key));
	app.context.set_singleton(runtime);
	// Act
	let identity = assert_json(app.anonymous.get("/.well-known/aidash").await.unwrap(), 200);
	// Assert
	assert_eq!(identity["key"]["id"], id.as_str());
	assert_eq!(identity["key"]["public_key_pem"], pem.as_str());
	assert_eq!(id.len(), 43);
	assert!(pem.starts_with("-----BEGIN PUBLIC KEY-----"));
	assert_eq!(
		fields(&identity["key"]),
		BTreeSet::from(["id", "public_key_pem"]),
		"the private key is never published"
	);
}
