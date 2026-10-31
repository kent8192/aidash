//! Portable Capability Token contracts. Private production keys stay in KMS.
use ed25519_dalek::VerifyingKey;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

pub mod federation;
mod jws;

pub const TOKEN_TTL_SECS: u64 = 60;
pub const TOKEN_TYPE: &str = "aidash-capability+jwt";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
	Chat,
	Discovery,
	Embeddings,
	/// One System One request to a Decider's provider (a Decision Attempt).
	/// It is always alone in `ops`, for a Run or approval-observation subject,
	/// with no output bound.
	Decision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Maintenance {
	MemoryIndexing,
	MemoryRetention,
	MemoryReflection,
	MemoryRetrieval,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum TokenSubject {
	Run {
		run: String,
		call: Uuid,
	},
	/// An independently leased Shadow observation, including Generation
	/// approvals that do not have an executing Run.
	ApprovalObservation {
		observation: Uuid,
		tenant: String,
		call: Uuid,
	},
	Maintenance {
		maintenance: Maintenance,
		tenant: String,
	},
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claims {
	pub iss: String,
	pub aud: String,
	pub iat: u64,
	pub exp: u64,
	pub jti: Uuid,
	pub kid: String,
	pub tenant: String,
	pub provider: String,
	pub credential: Uuid,
	pub version: String,
	pub sub: TokenSubject,
	pub ops: Vec<Operation>,
	pub model: String,
	pub max_output_tokens: u64,
	/// Authorizes `{"type":"ephemeral"}` prompt-cache breakpoints on message text
	/// parts (ADR 0019). Omitted from the wire when unset, so tokens without the
	/// authority keep their shape and older brokers still read them.
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub cache_breakpoints: bool,
	/// The OpenRouter hosts of the Decider Route a Decisions API request must
	/// name, in order, as `provider.only` (ADR 0066). Empty for every other
	/// token, including a System One Decision Attempt's, and then omitted.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub hosts: Vec<String>,
}

/// Safe, stable broker-owned failure codes. No rejected input is retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
	#[error("invalid_signature")]
	InvalidSignature,
	#[error("unknown_kid")]
	UnknownKid,
	#[error("expired")]
	Expired,
	#[error("audience")]
	Audience,
	#[error("tenant")]
	Tenant,
	#[error("credential")]
	Credential,
	#[error("operation")]
	Operation,
	#[error("model")]
	Model,
	#[error("claim_violation")]
	ClaimViolation,
}

impl From<jws::Rejection> for Failure {
	fn from(rejection: jws::Rejection) -> Self {
		match rejection {
			jws::Rejection::UnknownKid => Self::UnknownKid,
			jws::Rejection::Malformed | jws::Rejection::InvalidSignature => Self::InvalidSignature,
		}
	}
}

/// Signing is an external port; it takes the exact JWS input, never a digest.
#[async_trait::async_trait]
pub trait TokenSigner: Send + Sync {
	fn kid(&self) -> &str;
	async fn sign(&self, input: &[u8]) -> Result<[u8; 64], SignError>;
}
#[derive(Debug, thiserror::Error)]
#[error("Capability Token signing unavailable")]
pub struct SignError;

pub async fn mint(claims: &Claims, signer: &dyn TokenSigner) -> Result<SecretString, SignError> {
	jws::mint(claims, TOKEN_TYPE, &claims.kid, signer).await
}

/// The 32 raw bytes of an Ed25519 public key in SubjectPublicKeyInfo PEM, which
/// a Deployment Key ID digests.
pub fn public_key_bytes(pem: &str) -> Result<[u8; 32], SignError> {
	use ed25519_dalek::pkcs8::DecodePublicKey;
	Ok(VerifyingKey::from_public_key_pem(pem)
		.map_err(|_| SignError)?
		.to_bytes())
}

/// SubjectPublicKeyInfo PEM for a receiver's persisted Ed25519 public key.
pub fn public_key_pem(bytes: &[u8; 32]) -> Result<String, SignError> {
	use ed25519_dalek::pkcs8::{EncodePublicKey, spki::der::pem::LineEnding};
	VerifyingKey::from_bytes(bytes)
		.map_err(|_| SignError)?
		.to_public_key_pem(LineEnding::LF)
		.map_err(|_| SignError)
}

/// The broker uses only injected public keys. Verification makes no network call.
#[derive(Clone, Default)]
pub struct PublicKeys(BTreeMap<String, VerifyingKey>);
impl PublicKeys {
	pub fn insert(&mut self, kid: String, key: VerifyingKey) {
		self.0.insert(kid, key);
	}
	pub fn from_pems(keys: BTreeMap<String, String>) -> Result<Self, SignError> {
		use ed25519_dalek::pkcs8::DecodePublicKey;
		let mut result = Self::default();
		for (kid, pem) in keys {
			result.insert(
				kid,
				VerifyingKey::from_public_key_pem(&pem).map_err(|_| SignError)?,
			);
		}
		if result.0.is_empty() {
			return Err(SignError);
		}
		Ok(result)
	}
	/// `now` is supplied by admission; expiry is never checked during a stream.
	pub fn verify(
		&self,
		token: &str,
		issuer: &str,
		audience: &str,
		now: u64,
	) -> Result<Claims, Failure> {
		let (kid, claims): (String, Claims) =
			jws::verify(token, TOKEN_TYPE, self).map_err(Failure::from)?;
		if claims.kid != kid {
			return Err(Failure::UnknownKid);
		}
		if claims.iss != issuer || claims.aud != audience {
			return Err(Failure::Audience);
		}
		if now >= claims.exp {
			return Err(Failure::Expired);
		}
		if claims.iat > now
			|| claims.exp <= claims.iat
			|| claims.exp - claims.iat > TOKEN_TTL_SECS
			|| claims.jti.is_nil()
		{
			return Err(Failure::ClaimViolation);
		}
		if claims.tenant.is_empty() || claims.tenant.len() > 256 {
			return Err(Failure::Tenant);
		}
		match &claims.sub {
			TokenSubject::Run { run, call } if run.is_empty() || call.is_nil() => {
				return Err(Failure::ClaimViolation);
			}
			TokenSubject::Maintenance { tenant, .. } if tenant != &claims.tenant => {
				return Err(Failure::Tenant);
			}
			TokenSubject::ApprovalObservation { tenant, .. } if tenant != &claims.tenant => {
				return Err(Failure::Tenant);
			}
			TokenSubject::ApprovalObservation {
				observation, call, ..
			} if observation.is_nil() || call.is_nil() => {
				return Err(Failure::ClaimViolation);
			}
			_ => {}
		}
		if claims.credential.is_nil()
			|| claims.version.is_empty()
			|| claims.version.starts_with('0')
			|| !claims.version.bytes().all(|c| c.is_ascii_digit())
		{
			return Err(Failure::Credential);
		}
		if claims.ops.is_empty()
			|| claims.ops.len() > 3
			|| claims
				.ops
				.iter()
				.enumerate()
				.any(|(i, op)| claims.ops[..i].contains(op))
		{
			return Err(Failure::Operation);
		}
		if claims.model.split('/').any(|s| {
			s.is_empty()
				|| s == "." || s == ".."
				|| !s
					.bytes()
					.all(|c| c.is_ascii_alphanumeric() || b"-_.:".contains(&c))
		}) || claims.model.len() > 256
		{
			return Err(Failure::Model);
		}
		// A Decision Attempt returns probabilities, not completion tokens (ADR 0041).
		if claims.ops.contains(&Operation::Decision) {
			if claims.ops.len() != 1
				|| claims.max_output_tokens != 0
				|| claims.cache_breakpoints
				|| !matches!(
					claims.sub,
					TokenSubject::Run { .. } | TokenSubject::ApprovalObservation { .. }
				) {
				return Err(Failure::ClaimViolation);
			}
		} else if claims.max_output_tokens == 0
			|| !claims.hosts.is_empty()
			|| matches!(claims.sub, TokenSubject::ApprovalObservation { .. })
		{
			return Err(Failure::ClaimViolation);
		}
		Ok(claims)
	}
}

/// In-memory Ed25519 adapter for deterministic tests; never used by Cloud composition.
pub struct InMemorySigner {
	kid: String,
	key: ed25519_dalek::SigningKey,
}
impl InMemorySigner {
	pub fn new(kid: String, bytes: [u8; 32]) -> Self {
		Self {
			kid,
			key: ed25519_dalek::SigningKey::from_bytes(&bytes),
		}
	}
	pub fn public_key(&self) -> VerifyingKey {
		self.key.verifying_key()
	}
	/// The public key as a Deployment publishes it: SubjectPublicKeyInfo PEM.
	pub fn public_key_pem(&self) -> String {
		use ed25519_dalek::pkcs8::{EncodePublicKey, spki::der::pem::LineEnding};
		self.key
			.verifying_key()
			.to_public_key_pem(LineEnding::LF)
			.expect("an Ed25519 public key always encodes")
	}
}
#[async_trait::async_trait]
impl TokenSigner for InMemorySigner {
	fn kid(&self) -> &str {
		&self.kid
	}
	async fn sign(&self, input: &[u8]) -> Result<[u8; 64], SignError> {
		use ed25519_dalek::Signer;
		Ok(self.key.sign(input).to_bytes())
	}
}

#[cfg(test)]
mod tests;

/// The broker alone may read pinned Key Material. Errors never retain provider input.
#[async_trait::async_trait]
pub trait KeyMaterialSource: Send + Sync {
	async fn access(&self, secret: &str, version: &str) -> Result<SecretString, KeyMaterialError>;
}
#[derive(Clone, Copy, Debug, thiserror::Error)]
pub enum KeyMaterialError {
	#[error("Provider Credential unavailable")]
	Unavailable,
	#[error("Secret Store unavailable")]
	StoreUnavailable,
}

/// A redacting in-memory source for broker tests, never selected by the binary.
#[derive(Default)]
pub struct InMemoryKeyMaterialSource {
	values: std::sync::Mutex<BTreeMap<(String, String), SecretString>>,
}
impl InMemoryKeyMaterialSource {
	pub fn insert(&self, secret: String, version: String, value: SecretString) {
		self.values
			.lock()
			.expect("Key Material lock")
			.insert((secret, version), value);
	}
}
#[async_trait::async_trait]
impl KeyMaterialSource for InMemoryKeyMaterialSource {
	async fn access(&self, secret: &str, version: &str) -> Result<SecretString, KeyMaterialError> {
		self.values
			.lock()
			.map_err(|_| KeyMaterialError::StoreUnavailable)?
			.get(&(secret.to_owned(), version.to_owned()))
			.cloned()
			.ok_or(KeyMaterialError::Unavailable)
	}
}
