//! OpenRouter's OAuth PKCE code exchange (`POST /auth/keys`). Responses carry
//! Key Material, so bodies are never logged or copied into errors.
use aidash_application::{Error, Result, provider_authorization::Exchange};
use aidash_domain::provider_credentials::Provider;
use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

pub struct OpenRouterAuthorization {
	client: reqwest::Client,
	/// The Provider Catalog base URL; only test builds may point it elsewhere.
	base_url: Option<String>,
}
impl OpenRouterAuthorization {
	pub fn new(client: reqwest::Client) -> Self {
		Self {
			client,
			base_url: None,
		}
	}
	/// Points the exchange at a fake provider. Compiled only into tests and
	/// `test-support` builds, which no production dependency enables.
	#[cfg(any(test, feature = "test-support"))]
	pub fn at(client: reqwest::Client, base_url: String) -> Self {
		Self {
			client,
			base_url: Some(base_url),
		}
	}
}
#[derive(Deserialize)]
struct Issued {
	key: String,
}
fn rejected() -> Error {
	Error::Invalid("Provider Authorization was rejected".into())
}
fn unavailable() -> Error {
	Error::ProviderRejected {
		status: 503,
		reason: "Provider Authorization is temporarily unavailable".into(),
	}
}
#[async_trait]
impl Exchange for OpenRouterAuthorization {
	async fn exchange(
		&self,
		provider: Provider,
		code: &str,
		verifier: &SecretString,
	) -> Result<SecretString> {
		if code.is_empty() || code.len() > 1024 {
			return Err(rejected());
		}
		let base = self.base_url.as_deref().unwrap_or(provider.base_url());
		let response = self
			.client
			.post(format!("{base}/auth/keys"))
			.timeout(Duration::from_secs(15))
			.json(&json!({
				"code": code,
				"code_verifier": verifier.expose_secret(),
				"code_challenge_method": "S256",
			}))
			.send()
			.await
			.map_err(|_| unavailable())?;
		match response.status().as_u16() {
			200 | 201 => {}
			// Invalid code or verifier, expired code, or method mismatch.
			400 | 401 | 403 | 404 => return Err(rejected()),
			_ => return Err(unavailable()),
		}
		let issued: Issued = crate::response::json(response, 16_384)
			.await
			.map_err(|_| unavailable())?;
		let key = SecretString::from(issued.key);
		if key.expose_secret().trim().is_empty() {
			return Err(unavailable());
		}
		Ok(key)
	}
}
#[cfg(test)]
mod tests;
