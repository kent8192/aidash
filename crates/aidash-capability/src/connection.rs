//! Broker-owned OAuth grants. This token kind cannot authorize provider or
//! pasted-secret calls; all payloads bind credentials to their issuer/resource.
use crate::tenant_secret::{Attach, Destination, SecretClaims, SecretSubject};
use crate::{Failure, PublicKeys, SignError, TOKEN_TTL_SECS, TokenSigner, jws};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;
use zeroize::Zeroizing;

pub mod sealed;
pub const CONNECTION_TOKEN_TYPE: &str = "aidash-connection-call+jwt";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
	Registered,
	Dynamic,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientSecretRef {
	pub kind: ClientKind,
	pub id: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConnectionOperation {
	Exchange {
		issuer: String,
		resource: String,
		client_id: String,
		client: Option<ClientSecretRef>,
		redirect_uri: String,
		sub: SecretSubject,
	},
	Refresh {
		version: String,
		sub: SecretSubject,
	},
	Revoke {
		version: String,
	},
	Forward {
		version: String,
		destination: Destination,
		methods: Vec<String>,
		sub: SecretSubject,
		#[serde(default)]
		max_response_bytes: Option<u64>,
	},
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionClaims {
	pub iss: String,
	pub aud: String,
	pub iat: u64,
	pub exp: u64,
	pub jti: Uuid,
	pub kid: String,
	pub tenant: String,
	pub connection: Uuid,
	pub operation: ConnectionOperation,
}
impl ConnectionClaims {
	pub fn validate(&self) -> Result<(), Failure> {
		if self.tenant.is_empty() || self.tenant.len() > 256 {
			return Err(Failure::Tenant);
		}
		if self.connection.is_nil() {
			return Err(Failure::ClaimViolation);
		}
		let subject = match &self.operation {
			ConnectionOperation::Exchange {
				issuer,
				resource,
				client_id,
				client,
				redirect_uri,
				sub,
			} => {
				if !secure_uri(issuer)
					|| !secure_uri(resource)
					|| !secure_uri(redirect_uri)
					|| client_id.is_empty()
					|| client_id.len() > 4096
					|| client_id.chars().any(char::is_control)
					|| client.as_ref().is_some_and(|c| c.id.is_nil())
				{
					return Err(Failure::ClaimViolation);
				}
				if !matches!(sub, SecretSubject::Operation { .. }) {
					return Err(Failure::ClaimViolation);
				}
				sub
			}
			ConnectionOperation::Refresh { version, sub } => {
				if !version_valid(version) || !matches!(sub, SecretSubject::Operation { .. }) {
					return Err(Failure::ClaimViolation);
				}
				sub
			}
			ConnectionOperation::Revoke { version } => {
				return if version_valid(version) {
					Ok(())
				} else {
					Err(Failure::ClaimViolation)
				};
			}
			ConnectionOperation::Forward {
				version,
				destination,
				methods,
				sub,
				max_response_bytes,
			} => {
				if matches!(sub, SecretSubject::Operation { .. }) {
					return Err(Failure::ClaimViolation);
				}
				return SecretClaims {
					iss: self.iss.clone(),
					aud: self.aud.clone(),
					iat: self.iat,
					exp: self.exp,
					jti: self.jti,
					kid: self.kid.clone(),
					tenant: self.tenant.clone(),
					secret: self.connection,
					version: version.clone(),
					destination: destination.clone(),
					methods: methods.clone(),
					attach: Attach::Bearer,
					sub: sub.clone(),
					max_response_bytes: *max_response_bytes,
				}
				.validate();
			}
		};
		match subject {
			SecretSubject::Operation { tenant, operation }
				if tenant == &self.tenant && !operation.is_nil() =>
			{
				Ok(())
			}
			_ => Err(Failure::Tenant),
		}
	}
}
pub fn version_valid(v: &str) -> bool {
	!v.is_empty() && !v.starts_with('0') && v.bytes().all(|c| c.is_ascii_digit())
}
fn secure_uri(s: &str) -> bool {
	let Ok(u) = url::Url::parse(s) else {
		return false;
	};
	let loopback = match u.host() {
		Some(url::Host::Ipv4(i)) => i.is_loopback(),
		Some(url::Host::Ipv6(i)) => i.is_loopback(),
		_ => false,
	};
	(u.scheme() == "https" || (u.scheme() == "http" && loopback))
		&& u.host().is_some()
		&& u.username().is_empty()
		&& u.password().is_none()
		&& u.fragment().is_none()
		&& u.query().is_none()
		&& s.len() <= 8192
}
pub async fn mint_connection(
	claims: &ConnectionClaims,
	signer: &dyn TokenSigner,
) -> Result<SecretString, SignError> {
	jws::mint(claims, CONNECTION_TOKEN_TYPE, &claims.kid, signer).await
}
impl PublicKeys {
	pub fn verify_connection(
		&self,
		token: &str,
		issuers: &BTreeMap<String, String>,
		audience: &str,
		now: u64,
	) -> Result<ConnectionClaims, Failure> {
		let (kid, claims): (String, ConnectionClaims) =
			jws::verify(token, CONNECTION_TOKEN_TYPE, self).map_err(Failure::from)?;
		if claims.kid != kid {
			return Err(Failure::UnknownKid);
		}
		if issuers.get(&kid).ok_or(Failure::UnknownKid)? != &claims.iss || claims.aud != audience {
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
		claims.validate()?;
		Ok(claims)
	}
}
/// A broker-only read/write port: the application host has no payload reader.
#[async_trait::async_trait]
pub trait ConnectionStore: Send + Sync {
	async fn access(
		&self,
		resource: &str,
		version: &str,
	) -> Result<Zeroizing<Vec<u8>>, crate::KeyMaterialError>;
	async fn latest(
		&self,
		resource: &str,
	) -> Result<(String, Zeroizing<Vec<u8>>), crate::KeyMaterialError>;
	async fn add_version(
		&self,
		resource: &str,
		payload: &[u8],
	) -> Result<String, crate::KeyMaterialError>;
}
#[cfg(test)]
mod tests;
