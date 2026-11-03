//! One bounded predecessor-signed proof for a normal Deployment Key rollover.
use super::{FederationFailure, body_digest};
use crate::{PublicKeys, SignError, TokenSigner, jws};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::VerifyingKey;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};

pub const ROLLOVER_TOKEN_TYPE: &str = "aidash-deployment-key-transition+jwt";
pub const PUBLICATION_DELAY_SECS: u64 = 86_400;
pub const PREDECESSOR_OVERLAP_SECS: u64 = 604_800;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RolloverClaims {
	pub deployment: String,
	pub predecessor_kid: String,
	pub successor_kid: String,
	/// Base64url without padding of exactly 32 raw Ed25519 public-key bytes.
	pub successor_public_key: String,
	pub published_at: u64,
	pub activates_at: u64,
	pub retires_at: u64,
}

/// The receiver's CURRENT pinned key, never an arbitrary historical key set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustedRolloverKey {
	pub kid: String,
	pub public_key_pem: String,
}

impl RolloverClaims {
	/// Decode the already authenticated successor for an adapter's trust update.
	pub fn successor_key_bytes(&self) -> Result<[u8; 32], FederationFailure> {
		let bytes = URL_SAFE_NO_PAD
			.decode(&self.successor_public_key)
			.map_err(|_| FederationFailure::ClaimViolation)?;
		let bytes: [u8; 32] = bytes
			.try_into()
			.map_err(|_| FederationFailure::ClaimViolation)?;
		if body_digest(&bytes) != self.successor_kid {
			return Err(FederationFailure::ClaimViolation);
		}
		VerifyingKey::from_bytes(&bytes).map_err(|_| FederationFailure::ClaimViolation)?;
		Ok(bytes)
	}

	fn validate(&self) -> Result<(), FederationFailure> {
		let name = self
			.deployment
			.strip_prefix("aidash://")
			.unwrap_or_default();
		if name.is_empty()
			|| name.len() > 100
			|| !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
			|| self.predecessor_kid == self.successor_kid
			|| self.published_at.checked_add(PUBLICATION_DELAY_SECS) != Some(self.activates_at)
			|| self.activates_at.checked_add(PREDECESSOR_OVERLAP_SECS) != Some(self.retires_at)
		{
			return Err(FederationFailure::ClaimViolation);
		}
		self.successor_key_bytes()?;
		Ok(())
	}
}

pub async fn mint_transition(
	claims: &RolloverClaims,
	predecessor: &dyn TokenSigner,
) -> Result<String, SignError> {
	claims.validate().map_err(|_| SignError)?;
	let compact = jws::mint(
		claims,
		ROLLOVER_TOKEN_TYPE,
		&claims.predecessor_kid,
		predecessor,
	)
	.await?;
	let compact = compact.expose_secret();
	if compact.len() > jws::MAX_TOKEN_LEN {
		return Err(SignError);
	}
	Ok(compact.to_owned())
}

/// Authenticates a transition only during [activation, retirement). This alone
/// never admits a request or authorizes a persistent trust update: admission
/// must also authenticate the successor-signed outer Federation token.
pub fn verify_transition(
	compact: &str,
	trusted: &TrustedRolloverKey,
	deployment: &str,
	now: u64,
) -> Result<RolloverClaims, FederationFailure> {
	let bytes = crate::public_key_bytes(&trusted.public_key_pem)
		.map_err(|_| FederationFailure::ClaimViolation)?;
	if body_digest(&bytes) != trusted.kid {
		return Err(FederationFailure::ClaimViolation);
	}
	let mut keys = PublicKeys::default();
	keys.insert(
		trusted.kid.clone(),
		VerifyingKey::from_bytes(&bytes).map_err(|_| FederationFailure::ClaimViolation)?,
	);
	let (kid, claims): (String, RolloverClaims) = jws::verify(compact, ROLLOVER_TOKEN_TYPE, &keys)?;
	claims.validate()?;
	if kid != trusted.kid
		|| claims.predecessor_kid != kid
		|| claims.deployment != deployment
		|| now < claims.activates_at
		|| now >= claims.retires_at
	{
		return Err(FederationFailure::ClaimViolation);
	}
	Ok(claims)
}
