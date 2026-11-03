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

/// Initial receiver-local trust after an administrator confirmed the pin.
pub fn trusted_keys(key: &PinnedKey) -> Result<aidash_domain::nodes::keys::TrustedDeploymentKeys> {
	use aidash_domain::nodes::keys::{PublicDeploymentKey, TrustedDeploymentKeys};
	let bytes = aidash_capability::public_key_bytes(&key.public_key_pem)
		.map_err(|_| Error::Unauthorized)?;
	Ok(TrustedDeploymentKeys::new(PublicDeploymentKey::new(
		key.key_id.clone(),
		bytes,
	)?)?)
}

/// Returns a candidate only after the whole outer request authenticates. The
/// caller must still check its owner/subject claims and current authorization
/// before committing that candidate with both expected revisions.
pub(super) fn verify_request(
	raw: &str,
	deployment: &DeploymentId,
	trusted: &aidash_domain::nodes::keys::TrustedDeploymentKeys,
	now: u64,
	request: &aidash_capability::federation::RequestBinding<'_>,
) -> Result<(
	aidash_capability::federation::FederationClaims,
	Option<aidash_domain::nodes::keys::TrustedDeploymentKeys>,
)> {
	use aidash_capability::{
		PublicKeys,
		federation::{
			FederationClaims,
			rollover::{TrustedRolloverKey, verify_transition},
		},
	};
	use aidash_domain::nodes::keys::{PublicDeploymentKey, RolloverSchedule};
	use chrono::{DateTime, Utc};
	use std::collections::BTreeMap;
	fn refused<E>(_: E) -> Error {
		Error::Unauthorized
	}
	let instant = |seconds: u64| -> Result<DateTime<Utc>> {
		DateTime::from_timestamp(i64::try_from(seconds).map_err(refused)?, 0)
			.ok_or(Error::Unauthorized)
	};
	let timestamp = instant(now)?;
	if trusted.quarantined {
		return Err(Error::Unauthorized);
	}
	let kid = DeploymentKeyId::parse(&FederationClaims::unverified(raw).map_err(refused)?.kid)
		.map_err(refused)?;
	if let Some(key) = trusted.trusted_key(&kid, timestamp) {
		key.validate().map_err(refused)?;
		let pem = aidash_capability::public_key_pem(&key.public_key).map_err(refused)?;
		let keys =
			PublicKeys::from_pems(BTreeMap::from([(kid.to_string(), pem)])).map_err(refused)?;
		return Ok((
			FederationClaims::verify(raw, &keys, now, request).map_err(refused)?,
			None,
		));
	}
	trusted.current.validate().map_err(refused)?;
	let predecessor = TrustedRolloverKey {
		kid: trusted.current.key_id.to_string(),
		public_key_pem: aidash_capability::public_key_pem(&trusted.current.public_key)
			.map_err(refused)?,
	};
	let claims = FederationClaims::verify_with_transition(
		raw,
		&predecessor,
		deployment.as_str(),
		now,
		request,
	)
	.map_err(refused)?;
	let transition = verify_transition(
		claims.transition.as_deref().ok_or(Error::Unauthorized)?,
		&predecessor,
		deployment.as_str(),
		now,
	)
	.map_err(refused)?;
	let successor = PublicDeploymentKey::new(
		DeploymentKeyId::parse(&transition.successor_kid).map_err(refused)?,
		transition.successor_key_bytes().map_err(refused)?,
	)
	.map_err(refused)?;
	let schedule = RolloverSchedule {
		published_at: instant(transition.published_at)?,
		activates_at: instant(transition.activates_at)?,
		retires_at: instant(transition.retires_at)?,
	};
	let mut advanced = trusted.clone();
	advanced
		.accept_successor(&trusted.current.key_id, successor, schedule, timestamp)
		.map_err(refused)?;
	Ok((claims, Some(advanced)))
}

#[cfg(test)]
mod tests;
