//! Provider Authorization: the provider-hosted OAuth PKCE consent that is the
//! only way to obtain a Provider Credential (ADR 0006). The host keeps the
//! `state` and PKCE verifier, and the code exchange returns Key Material that
//! goes straight into the Provider Credential Store write path.
use crate::{Error, Result};
use aidash_domain::provider_credentials::Provider;
use async_trait::async_trait;
use base64::Engine;
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// How long a pending authorization waits for its callback. OpenRouter's code
/// expires ten minutes after it is issued, which happens only once the user
/// approves, and the exchange enforces that bound. This window therefore also
/// covers the time the user spends on the consent page.
pub const LIFETIME_MINUTES: i64 = 30;
const KEY_LABEL_MAX_CHARS: usize = 64;

/// Exchanges one authorization code for Key Material. Implementations must
/// never log, event or return the provider's response body in errors.
#[async_trait]
pub trait Exchange: Send + Sync {
	async fn exchange(
		&self,
		provider: Provider,
		code: &str,
		verifier: &SecretString,
	) -> Result<SecretString>;
}

/// The host-held half of one authorization: `state` binds the callback to the
/// pending record and the verifier proves possession at exchange time.
pub struct Request {
	pub state: SecretString,
	pub verifier: SecretString,
	pub url: String,
}

pub(crate) fn random_secret() -> String {
	// Two UUIDv4 values carry 244 random bits; hex is a PKCE-unreserved alphabet.
	format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// RFC 7636 `S256`: unpadded base64url SHA-256 of the verifier.
pub fn challenge(verifier: &SecretString) -> String {
	base64::engine::general_purpose::URL_SAFE_NO_PAD
		.encode(Sha256::digest(verifier.expose_secret().as_bytes()))
}

/// The label shown for the issued key in the provider's key list.
pub fn key_label(tenant: &str) -> String {
	let label = format!("Aidash · {tenant}");
	label.chars().take(KEY_LABEL_MAX_CHARS).collect()
}

/// Starts one authorization for `provider`, returning the consent URL to
/// which the browser is sent. The callback must be the host's own URL.
pub fn begin(provider: Provider, callback_url: &str, tenant: &str) -> Result<Request> {
	// Plan Connections use Sign in with ChatGPT, never this flow.
	provider.require_credential()?;
	let callback = url::Url::parse(callback_url)
		.map_err(|_| Error::Invalid("invalid Provider Authorization callback URL".into()))?;
	if !matches!(callback.scheme(), "http" | "https")
		|| callback.host_str().is_none()
		|| callback.query().is_some()
		|| callback.fragment().is_some()
	{
		return Err(Error::Invalid(
			"invalid Provider Authorization callback URL".into(),
		));
	}
	let state = SecretString::from(random_secret());
	let verifier = SecretString::from(random_secret());
	let mut url = url::Url::parse(provider.authorization_url())
		.map_err(|_| Error::Invalid("invalid Provider Authorization URL".into()))?;
	url.query_pairs_mut()
		.append_pair("callback_url", callback.as_str())
		.append_pair("code_challenge", &challenge(&verifier))
		.append_pair("code_challenge_method", "S256")
		.append_pair("key_label", &key_label(tenant))
		.append_pair("state", state.expose_secret());
	Ok(Request {
		state,
		verifier,
		url: url.into(),
	})
}

/// Unkeyed SHA-256 (hex) of Key Material for the provider's key-settings deep
/// link; see [`aidash_domain::provider_credentials::ProviderCredential::key_digest`].
pub fn key_digest(key: &SecretString) -> String {
	Sha256::digest(key.expose_secret().as_bytes())
		.iter()
		.map(|b| format!("{b:02x}"))
		.collect()
}

/// Which Provider Credential one authorization writes. A reconnect adds a new
/// version to an existing record (rotation) at its expected revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
	Connect { id: Uuid },
	Reconnect { id: Uuid, expected_revision: i64 },
}
impl Target {
	pub fn id(self) -> Uuid {
		match self {
			Self::Connect { id } | Self::Reconnect { id, .. } => id,
		}
	}
}

#[cfg(test)]
mod tests;
