//! The server's own Tenant Secret Capability Token minting (ADR 0095). It
//! signs with a KMS key of its own, never the worker's, so the broker can tell
//! the two issuers apart and stop either alone.
use aidash_application::{Error, Result};
use aidash_capability::{
	TOKEN_TTL_SECS, TokenSigner,
	tenant_secret::{Attach, Destination, SecretClaims, SecretSubject},
};
use secrecy::SecretString;
use std::{
	sync::Arc,
	time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretIssuerConfiguration {
	pub issuer: String,
	pub audience: String,
	pub kid: String,
}
impl SecretIssuerConfiguration {
	pub fn validate(&self) -> Result<()> {
		if self.issuer.is_empty()
			|| self.issuer.len() > 256
			|| self.audience.is_empty()
			|| self.audience.len() > 64
			|| !self
				.audience
				.bytes()
				.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
			|| self.kid.is_empty()
		{
			return Err(Error::Invalid(
				"invalid Tenant Secret Capability Token issuer configuration".into(),
			));
		}
		Ok(())
	}
}

/// What a caller of a Tenant Secret supplies; the issuer stamps `iss`, `aud`,
/// `iat`, `exp`, `jti` and `kid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRequest {
	pub tenant: String,
	pub secret: Uuid,
	pub version: String,
	pub destination: Destination,
	pub methods: Vec<String>,
	pub attach: Attach,
	pub sub: SecretSubject,
	pub max_response_bytes: Option<u64>,
}

pub struct SecretTokenIssuer {
	configuration: SecretIssuerConfiguration,
	signer: Arc<dyn TokenSigner>,
}
impl SecretTokenIssuer {
	pub fn new(
		configuration: SecretIssuerConfiguration,
		signer: Arc<dyn TokenSigner>,
	) -> Result<Self> {
		configuration.validate()?;
		if configuration.kid != signer.kid() {
			return Err(Error::Invalid(
				"the Tenant Secret Capability Token signer does not match its configuration".into(),
			));
		}
		Ok(Self {
			configuration,
			signer,
		})
	}

	pub async fn mint(&self, request: &SecretRequest) -> Result<SecretString> {
		let now = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.map_err(|_| Error::Invalid("Tenant Secret Capability Token clock unavailable".into()))?
			.as_secs();
		let claims = SecretClaims {
			iss: self.configuration.issuer.clone(),
			aud: self.configuration.audience.clone(),
			iat: now,
			exp: now + TOKEN_TTL_SECS,
			jti: Uuid::new_v4(),
			kid: self.configuration.kid.clone(),
			tenant: request.tenant.clone(),
			secret: request.secret,
			version: request.version.clone(),
			destination: request.destination.clone(),
			methods: request.methods.clone(),
			attach: request.attach.clone(),
			sub: request.sub.clone(),
			max_response_bytes: request.max_response_bytes,
		};
		claims
			.validate()
			.map_err(|_| Error::Invalid("invalid Tenant Secret Capability Token claims".into()))?;
		let token = aidash_capability::tenant_secret::mint_secret(&claims, self.signer.as_ref())
			.await
			.map_err(|_| {
				Error::External("Tenant Secret Capability Token signing unavailable".into())
			})?;
		tracing::info!(
			jti = %claims.jti,
			tenant = %claims.tenant,
			secret = %claims.secret,
			"Tenant Secret Capability Token minted"
		);
		Ok(token)
	}
}

#[cfg(test)]
mod tests;
