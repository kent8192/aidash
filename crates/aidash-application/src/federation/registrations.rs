//! Registering a Node of another Deployment pins that Deployment's key (ADR 0064).
use crate::{Error, Result, ports::federation::registrations::DeploymentDirectory};
use aidash_domain::{
	federation::protocol::CURRENT,
	nodes::{DeploymentId, DeploymentKeyId, registration::PinnedKey},
};
use serde_json::Value;

/// Fetches the well-known document at `endpoint` and pins the key it names for
/// `deployment`.
/// The typed `key_id` is the trust anchor: a document whose key does not hash
/// to it is refused, so a stranger answering at `endpoint` pins nothing.
pub async fn pin(
	directory: &dyn DeploymentDirectory,
	deployment: &DeploymentId,
	endpoint: &str,
	key_id: &DeploymentKeyId,
) -> Result<PinnedKey> {
	let document = directory.identity(endpoint).await?;
	let mismatch =
		|what: &str| Error::Unprocessable(format!("the Deployment at {endpoint}: {what}"));
	if document["id"] != deployment.as_str() {
		return Err(mismatch("its id is not the Deployment named"));
	}
	if document["protocol_version"] != CURRENT {
		return Err(mismatch(&format!("it does not speak protocol {CURRENT}")));
	}
	let key = document["key"]
		.as_object()
		.ok_or_else(|| mismatch("it publishes no Deployment Key"))?;
	if key.get("id").and_then(Value::as_str) != Some(key_id.as_str()) {
		return Err(mismatch("its key ID is not the one given"));
	}
	let public_key_pem = key
		.get("public_key_pem")
		.and_then(Value::as_str)
		.ok_or_else(|| mismatch("its key has no public key"))?;
	let bytes = aidash_capability::public_key_bytes(public_key_pem)
		.map_err(|_| mismatch("its public key is not an Ed25519 key"))?;
	if DeploymentKeyId::of_public_key(&bytes) != *key_id {
		return Err(mismatch("its public key is not the one its key ID names"));
	}
	Ok(PinnedKey {
		key_id: key_id.clone(),
		public_key_pem: public_key_pem.to_owned(),
	})
}

#[cfg(test)]
mod tests;
