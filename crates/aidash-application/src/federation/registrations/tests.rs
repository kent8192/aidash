use super::*;
use aidash_capability::InMemorySigner;
use async_trait::async_trait;
use serde_json::json;

struct Answers(Value);

#[async_trait]
impl DeploymentDirectory for Answers {
	async fn identity(&self, _: &str) -> Result<Value> {
		Ok(self.0.clone())
	}
}

const SOURCE: &str = "aidash://calm-heron/able-oak-abcd1234";

fn signer(seed: u8) -> InMemorySigner {
	InMemorySigner::new("kid".into(), [seed; 32])
}

fn key_id(signer: &InMemorySigner) -> DeploymentKeyId {
	DeploymentKeyId::of_public_key(&signer.public_key().to_bytes())
}

fn document(signer: &InMemorySigner) -> Value {
	json!({
		"id": "aidash://calm-heron",
		"endpoint": "https://calm.example.com",
		"protocol_version": CURRENT,
		"key": {"id": key_id(signer).as_str(), "public_key_pem": signer.public_key_pem()},
	})
}

async fn pinned(document: Value, typed: &DeploymentKeyId) -> Result<PinnedKey> {
	pin(
		&Answers(document),
		&aidash_domain::nodes::NodeAddress::parse(SOURCE)
			.unwrap()
			.deployment,
		"https://calm.example.com",
		typed,
	)
	.await
}

#[tokio::test]
async fn a_document_that_names_the_typed_key_pins_it() {
	let signer = signer(1);
	let pinned = pinned(document(&signer), &key_id(&signer)).await.unwrap();
	assert_eq!(pinned.key_id, key_id(&signer));
	assert_eq!(pinned.public_key_pem, signer.public_key_pem());
}

#[tokio::test]
async fn every_way_the_document_can_disagree_is_refused_with_a_reason() {
	let signer = signer(1);
	let other = self::signer(2);
	let typed = key_id(&signer);
	let mut wrong_id = document(&signer);
	wrong_id["id"] = "aidash://someone-else".into();
	let mut old_protocol = document(&signer);
	old_protocol["protocol_version"] = "0.3".into();
	let mut no_key = document(&signer);
	no_key["key"] = Value::Null;
	let mut other_key_id = document(&signer);
	other_key_id["key"]["id"] = key_id(&other).as_str().into();
	let mut other_pem = document(&signer);
	other_pem["key"]["public_key_pem"] = other.public_key_pem().into();
	let mut not_a_key = document(&signer);
	not_a_key["key"]["public_key_pem"] = "nonsense".into();
	for (name, document) in [
		("id", wrong_id),
		("protocol", old_protocol),
		("no key", no_key),
		("key id", other_key_id),
		("pem under the typed id", other_pem),
		("pem", not_a_key),
	] {
		let refused = pinned(document, &typed).await.unwrap_err();
		assert!(
			matches!(refused, Error::Unprocessable(_)),
			"{name}: {refused:?}"
		);
	}
}
