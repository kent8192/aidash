//! Desktop broker use cases keep atomic persistence behind a port.
use crate::{Error, Result};
use aidash_domain::identity::desktop::{REFRESH_PREFIX, valid_secret};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Start {
	pub redirect_uri: String,
	pub state: String,
	pub code_challenge: String,
}
#[derive(Serialize, JsonSchema)]
pub struct Started {
	pub authorization_url: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Exchange {
	pub code: String,
	pub state: String,
	pub verifier: String,
	pub redirect_uri: String,
}
#[derive(Serialize, JsonSchema)]
pub struct Tokens {
	pub access_token: String,
	pub refresh_token: String,
	pub expires_in: i64,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Renewal {
	pub refresh_token: String,
	pub next_token: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Revocation {
	pub refresh_token: String,
}

pub async fn start(
	port: &dyn crate::ports::authorization::desktop::DesktopProtocol,
	input: Start,
) -> Result<Started> {
	let callback = url::Url::parse(&input.redirect_uri)
		.map_err(|_| Error::Invalid("invalid desktop callback".into()))?;
	if callback.scheme() != "http"
		|| callback.host_str() != Some("127.0.0.1")
		|| callback.port().is_none()
		|| !callback.username().is_empty()
		|| callback.password().is_some()
		|| callback.query().is_some()
		|| callback.fragment().is_some()
		|| callback.path() != "/callback"
	{
		return Err(Error::Invalid(
			"desktop callback must use an explicit loopback port and /callback".into(),
		));
	}
	if !valid_secret(&input.state)
		|| input.code_challenge.len() != 43
		|| URL_SAFE_NO_PAD
			.decode(&input.code_challenge)
			.map_or(true, |value| value.len() != 32)
	{
		return Err(Error::Invalid(
			"invalid desktop state or S256 challenge".into(),
		));
	}
	port.start(input).await
}

pub async fn exchange(
	port: &dyn crate::ports::authorization::desktop::DesktopProtocol,
	input: Exchange,
) -> Result<Tokens> {
	if !valid_secret(&input.code) || !valid_secret(&input.verifier) {
		return Err(Error::Unauthorized);
	}
	port.exchange(input).await
}

pub async fn refresh(
	port: &dyn crate::ports::authorization::desktop::DesktopProtocol,
	input: Renewal,
) -> Result<Tokens> {
	if !input.refresh_token.starts_with(REFRESH_PREFIX)
		|| !input.next_token.starts_with(REFRESH_PREFIX)
		|| !valid_secret(&input.refresh_token)
		|| !valid_secret(&input.next_token)
		|| input.refresh_token == input.next_token
	{
		return Err(Error::Unauthorized);
	}
	port.refresh(input).await
}

pub async fn revoke(
	port: &dyn crate::ports::authorization::desktop::DesktopProtocol,
	input: Revocation,
) -> Result<()> {
	port.revoke(input).await
}
