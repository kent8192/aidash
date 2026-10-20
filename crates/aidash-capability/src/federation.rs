//! Federation tokens (ADR 0064). A Node of one Deployment sends one to a Node
//! of another, signed with the sending Deployment's Deployment Key. It names
//! the sending and receiving Nodes and the sending subject, and it is bound to
//! one request, so a captured token reaches no other endpoint.
use crate::{PublicKeys, SignError, TOKEN_TTL_SECS, TokenSigner, jws};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const FEDERATION_TOKEN_TYPE: &str = "aidash-federation+jwt";

/// How far ahead of the receiver's clock a sender's `iat` may be. Two
/// Deployments keep their own clocks, and the 60 second lifetime leaves no
/// margin of its own.
pub const IAT_LEEWAY_SECS: u64 = 30;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationClaims {
	/// The sending Deployment id.
	pub iss: String,
	/// The sending Node address, or the viewer's Deployment id for a Peer
	/// Graph Grant, which has no Node.
	pub sub: String,
	/// The receiving Node address, or the receiving Deployment id for a Peer
	/// Graph Grant.
	pub aud: String,
	/// The sending subject's key in the sending Tenant's policy.
	pub subject: String,
	pub iat: u64,
	pub exp: u64,
	pub jti: Uuid,
	pub kid: String,
	/// The protocol version the sender speaks.
	pub protocol: String,
	pub method: String,
	/// The request target: the path under `/federation/v0.1` with its query.
	pub path: String,
	/// `body_digest` of the request body.
	pub body: String,
}

/// The request a token is checked against.
#[derive(Clone, Copy, Debug)]
pub struct RequestBinding<'a> {
	/// The protocol version the receiver speaks.
	pub protocol: &'a str,
	pub method: &'a str,
	pub path: &'a str,
	pub body: &'a [u8],
}

/// Why a Federation token was refused. No rejected input is retained, and a
/// receiver answers every one of them the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FederationFailure {
	#[error("malformed")]
	Malformed,
	#[error("unknown_kid")]
	UnknownKid,
	#[error("invalid_signature")]
	InvalidSignature,
	#[error("expired")]
	Expired,
	#[error("claim_violation")]
	ClaimViolation,
	#[error("protocol")]
	Protocol,
	#[error("request")]
	Request,
}

impl From<jws::Rejection> for FederationFailure {
	fn from(rejection: jws::Rejection) -> Self {
		match rejection {
			jws::Rejection::Malformed => Self::Malformed,
			jws::Rejection::UnknownKid => Self::UnknownKid,
			jws::Rejection::InvalidSignature => Self::InvalidSignature,
		}
	}
}

/// base64url without padding of SHA-256 over the request body bytes.
pub fn body_digest(body: &[u8]) -> String {
	URL_SAFE_NO_PAD.encode(Sha256::digest(body))
}

impl FederationClaims {
	/// Mint a token for one request, right before it is sent.
	pub async fn mint(&self, signer: &dyn TokenSigner) -> Result<SecretString, SignError> {
		jws::mint(self, FEDERATION_TOKEN_TYPE, &self.kid, signer).await
	}

	/// The claims of a token whose signature has not been checked. A receiver
	/// reads `aud` and `sub` this way to find the key to verify with; it must
	/// never act on any value before `verify` accepts the token.
	pub fn unverified(token: &str) -> Result<Self, FederationFailure> {
		let parts = jws::split(token, FEDERATION_TOKEN_TYPE)?;
		Ok(jws::decode(parts.payload)?)
	}

	/// Verify `token` against the keys the receiver trusts for its sender, at
	/// `now` seconds since the epoch, for exactly `request`. The checks run in
	/// this order: size, shape, header, key, signature, `kid`, expiry, issue
	/// time (at most `IAT_LEEWAY_SECS` ahead of `now`), lifetime, `jti`,
	/// protocol, then method, path and body. A repeat of
	/// the same request within the lifetime is the handlers' to deduplicate; the
	/// receiver keeps no `jti` store.
	pub fn verify(
		token: &str,
		keys: &PublicKeys,
		now: u64,
		request: &RequestBinding<'_>,
	) -> Result<Self, FederationFailure> {
		let (kid, claims): (String, Self) = jws::verify(token, FEDERATION_TOKEN_TYPE, keys)?;
		if claims.kid != kid {
			return Err(FederationFailure::ClaimViolation);
		}
		if now >= claims.exp {
			return Err(FederationFailure::Expired);
		}
		if claims.iat > now.saturating_add(IAT_LEEWAY_SECS)
			|| claims.exp <= claims.iat
			|| claims.exp - claims.iat > TOKEN_TTL_SECS
			|| claims.jti.is_nil()
		{
			return Err(FederationFailure::ClaimViolation);
		}
		if claims.protocol != request.protocol {
			return Err(FederationFailure::Protocol);
		}
		if claims.method != request.method
			|| claims.path != request.path
			|| claims.body != body_digest(request.body)
		{
			return Err(FederationFailure::Request);
		}
		Ok(claims)
	}
}

#[cfg(test)]
mod tests;
