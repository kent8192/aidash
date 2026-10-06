//! Provider account state is distinct from its persisted ORM representation.
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Account {
	pub id: Uuid,
	pub issuer: String,
	pub subject: String,
	pub last_valid_at: Option<DateTime<Utc>>,
	pub disabled_at: Option<DateTime<Utc>>,
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
