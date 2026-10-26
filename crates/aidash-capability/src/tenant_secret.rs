//! Tenant Secret Capability Tokens (ADR 0095): a kind of token of its own that
//! names one Tenant Secret at one version and the only destination it may reach.
//! The broker never accepts one kind as the other.
use crate::{Failure, PublicKeys, SignError, TOKEN_TTL_SECS, TokenSigner, jws};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

pub mod sealed;

/// The JWS `typ` of a token that authorizes one call that carries a Tenant Secret.
pub const SECRET_TOKEN_TYPE: &str = "aidash-secret-call+jwt";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Destination {
	/// `https://host[:port]`, lowercase, without a path.
	pub origin: String,
	/// Starts and ends with `/`; the broker forwards only paths under it.
	pub path_prefix: String,
}

/// How the broker attaches the value to the forwarded request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Attach {
	/// `Authorization: Bearer <value>`.
	Bearer,
	/// `<name>: <value>`; the name follows the External service header rules.
	Header { name: String },
}
impl Attach {
	pub fn header(&self) -> &str {
		match self {
			Self::Bearer => "authorization",
			Self::Header { name } => name,
		}
	}
}

/// What a call is made for. A subject that names a Tenant must name the token's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum SecretSubject {
	/// A tool call in a Run.
	Run { run: String, call: Uuid },
	/// An Egress operation, a Live preview or a stdio session.
	Operation { operation: Uuid, tenant: String },
	/// A Workbench test call.
	Workbench { tenant: String, call: Uuid },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretClaims {
	pub iss: String,
	pub aud: String,
	pub iat: u64,
	pub exp: u64,
	pub jti: Uuid,
	pub kid: String,
	pub tenant: String,
	/// The Tenant Secret id.
	pub secret: Uuid,
	/// The pinned Secret Store version.
	pub version: String,
	pub destination: Destination,
	pub methods: Vec<String>,
	pub attach: Attach,
	pub sub: SecretSubject,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_response_bytes: Option<u64>,
}
impl SecretClaims {
	/// The checks that need neither the signature nor the issuer, audience or
	/// clock. `PublicKeys::verify_secret` adds those, and an issuer may use this
	/// to refuse claims a broker would reject.
	pub fn validate(&self) -> Result<(), Failure> {
		if self.tenant.is_empty() || self.tenant.len() > 256 {
			return Err(Failure::Tenant);
		}
		if self.secret.is_nil() || !version_valid(&self.version) {
			return Err(Failure::ClaimViolation);
		}
		match &self.sub {
			SecretSubject::Run { run, call } if run.is_empty() || call.is_nil() => {
				return Err(Failure::ClaimViolation);
			}
			SecretSubject::Operation { operation, .. } if operation.is_nil() => {
				return Err(Failure::ClaimViolation);
			}
			SecretSubject::Operation { tenant, .. } | SecretSubject::Workbench { tenant, .. }
				if tenant != &self.tenant =>
			{
				return Err(Failure::Tenant);
			}
			SecretSubject::Workbench { call, .. } if call.is_nil() => {
				return Err(Failure::ClaimViolation);
			}
			_ => {}
		}
		if !origin_valid(&self.destination.origin)
			|| !path_prefix_valid(&self.destination.path_prefix)
		{
			return Err(Failure::Destination);
		}
		if !methods_valid(&self.methods) {
			return Err(Failure::Method);
		}
		if let Attach::Header { name } = &self.attach
			&& !header_valid(name)
		{
			return Err(Failure::ClaimViolation);
		}
		Ok(())
	}
}

pub async fn mint_secret(
	claims: &SecretClaims,
	signer: &dyn TokenSigner,
) -> Result<SecretString, SignError> {
	jws::mint(claims, SECRET_TOKEN_TYPE, &claims.kid, signer).await
}

impl PublicKeys {
	/// A Tenant Secret token, with the same structural checks as `verify`. `now`
	/// is supplied by admission; expiry is never checked during a stream.
	pub fn verify_secret(
		&self,
		token: &str,
		issuer: &str,
		audience: &str,
		now: u64,
	) -> Result<SecretClaims, Failure> {
		let (kid, claims): (String, SecretClaims) =
			jws::verify(token, SECRET_TOKEN_TYPE, self).map_err(Failure::from)?;
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
		claims.validate()?;
		Ok(claims)
	}
}

/// `https://host[:port]`, lowercase, with neither credentials nor a path.
fn origin_valid(origin: &str) -> bool {
	let Some(authority) = origin.strip_prefix("https://") else {
		return false;
	};
	if authority.is_empty()
		|| !origin.is_ascii()
		|| origin.bytes().any(|b| b.is_ascii_uppercase())
		|| authority.contains(['/', '?', '#', '@', '\\'])
		|| authority
			.bytes()
			.any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
	{
		return false;
	}
	let Ok(url) = url::Url::parse(origin) else {
		return false;
	};
	url.scheme() == "https"
		&& url.host_str().is_some_and(|host| !host.is_empty())
		&& url.username().is_empty()
		&& url.password().is_none()
		&& url.query().is_none()
		&& url.fragment().is_none()
		&& url.path() == "/"
}

fn path_prefix_valid(prefix: &str) -> bool {
	prefix.len() <= 2048
		&& prefix.starts_with('/')
		&& prefix.ends_with('/')
		&& !prefix.contains(['?', '#', '\\'])
		&& !prefix.bytes().any(|b| b.is_ascii_control())
		&& !prefix
			.split('/')
			.any(|segment| matches!(segment, "." | ".."))
}

fn methods_valid(methods: &[String]) -> bool {
	if methods.is_empty() {
		return false;
	}
	let mut seen = BTreeSet::new();
	methods.iter().all(|method| {
		!method.is_empty()
			&& method != "CONNECT"
			&& method != "TRACE"
			&& method
				.bytes()
				.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-')
			&& seen.insert(method.as_str())
	})
}

/// The same header-name rules as `ExternalService::validate`.
fn header_valid(name: &str) -> bool {
	let name = name.to_ascii_lowercase();
	!name.is_empty()
		&& name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
		&& !matches!(
			name.as_str(),
			"host"
				| "connection"
				| "transfer-encoding"
				| "content-length"
				| "cookie" | "proxy-authorization"
				| "upgrade" | "te"
				| "trailer" | "expect"
		) && !name.starts_with("sec-")
		&& !name.starts_with("x-forwarded-")
}

fn version_valid(version: &str) -> bool {
	!version.is_empty() && !version.starts_with('0') && version.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests;
