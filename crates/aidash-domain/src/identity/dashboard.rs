//! Provider account state is distinct from its persisted ORM representation.
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Account {
	pub id: Uuid,
	pub issuer: String,
	pub gcip_tenant: Option<String>,
	pub subject: String,
	pub valid_since: Option<DateTime<Utc>>,
	pub last_valid_at: Option<DateTime<Utc>>,
	pub disabled_at: Option<DateTime<Utc>>,
}

/// A GCIP Tenant is a user pool, never an authorization Tenant by itself.
pub fn bound_tenant<'a>(
	bindings: &'a std::collections::BTreeMap<String, String>,
	gcip_tenant: Option<&str>,
) -> Option<&'a str> {
	gcip_tenant
		.and_then(|tenant| bindings.get(tenant))
		.map(String::as_str)
}

/// Verified display attributes never establish authority or link identities.
#[derive(Debug, Clone)]
pub struct SignIn {
	pub subject: String,
	pub gcip_tenant: Option<String>,
	/// The sign-in method from a verified GCIP token, independent of UI choices.
	pub gcip_provider: Option<String>,
	pub auth_time: DateTime<Utc>,
	pub verified_email: Option<String>,
	pub display_name: Option<String>,
}

/// Only the issuer-reported value drives session revocation; events that advance
/// it must be verified in a GCIP sandbox, never inferred from account attributes.
#[derive(Debug, Clone, Copy, Default)]
pub struct AccountState {
	pub disabled: bool,
	pub valid_since: Option<DateTime<Utc>>,
}

pub fn session_revoked(auth_time: DateTime<Utc>, valid_since: Option<DateTime<Utc>>) -> bool {
	valid_since.is_some_and(|since| auth_time < since)
}

/// Replay identity is consumed atomically with the matching session revocations.
pub struct LogoutClaims {
	pub subject: Option<String>,
	pub session: Option<String>,
	pub jti: String,
	pub expires_at: DateTime<Utc>,
}

pub fn logout_claims(claims: &serde_json::Value, now: i64) -> crate::Result<LogoutClaims> {
	use crate::Error;
	use serde_json::Value;

	let object = claims
		.as_object()
		.ok_or(Error::Invalid("invalid logout token claims".into()))?;
	let event = object
		.get("events")
		.and_then(Value::as_object)
		.and_then(|events| events.get("http://schemas.openid.net/event/backchannel-logout"));
	if !event.is_some_and(Value::is_object) || object.contains_key("nonce") {
		return Err(Error::Invalid("invalid logout token claims".into()));
	}
	let sub = object
		.get("sub")
		.and_then(Value::as_str)
		.filter(|value| !value.is_empty());
	let sid = object
		.get("sid")
		.and_then(Value::as_str)
		.filter(|value| !value.is_empty());
	if sub.is_none() && sid.is_none() {
		return Err(Error::Invalid(
			"logout token needs a subject or session ID".into(),
		));
	}
	let jti = object
		.get("jti")
		.and_then(Value::as_str)
		.filter(|value| !value.is_empty() && value.len() <= 512)
		.ok_or(Error::Invalid("logout token needs a JTI".into()))?;
	let iat = object
		.get("iat")
		.and_then(Value::as_i64)
		.ok_or(Error::Invalid("invalid logout issued time".into()))?;
	let exp = object
		.get("exp")
		.and_then(Value::as_i64)
		.ok_or(Error::Invalid("invalid logout expiration".into()))?;
	if iat > now + 30 || iat < now - 86_400 || exp <= now - 30 || exp > now + 86_400 || exp <= iat {
		return Err(Error::Invalid("invalid logout token lifetime".into()));
	}
	Ok(LogoutClaims {
		subject: sub.map(str::to_owned),
		session: sid.map(str::to_owned),
		jti: jti.to_owned(),
		expires_at: DateTime::from_timestamp(exp, 0)
			.ok_or(Error::Invalid("invalid logout expiration".into()))?,
	})
}

#[cfg(test)]
mod tests;
