//! Fixed-catalog Provider Credential key validation.
use aidash_application::{
	Error, Result,
	provider_credentials::{KeyValidator, Validation},
};
use aidash_domain::provider_credentials::Provider;
use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use std::time::Duration;

pub struct OpenRouterKeyValidator {
	pub client: reqwest::Client,
}
#[async_trait]
impl KeyValidator for OpenRouterKeyValidator {
	async fn validate(&self, provider: Provider, key: &SecretString) -> Result<Validation> {
		self.validate_at(&format!("{}/key", provider.base_url()), key)
			.await
	}
}
impl OpenRouterKeyValidator {
	async fn validate_at(&self, url: &str, key: &SecretString) -> Result<Validation> {
		let response = self
			.client
			.get(url)
			.timeout(Duration::from_secs(15))
			.bearer_auth(key.expose_secret())
			.send()
			.await
			.map_err(|_| Error::ProviderRejected {
				status: 503,
				reason: "Provider Credential validation is temporarily unavailable".into(),
			})?;
		match response.status().as_u16() {
			401 | 403 => {
				return Err(Error::Invalid(
					"invalid Provider Credential Key Material".into(),
				));
			}
			429 | 500..=599 => {
				return Err(Error::ProviderRejected {
					status: 503,
					reason: "Provider Credential validation is temporarily unavailable".into(),
				});
			}
			200 => {}
			_ => {
				return Err(Error::Invalid(
					"Provider Credential validation was rejected".into(),
				));
			}
		}
		let value: Value =
			crate::response::json(response, 16384)
				.await
				.map_err(|_| Error::ProviderRejected {
					status: 503,
					reason: "Provider Credential validation response is unavailable".into(),
				})?;
		let data = value.get("data").filter(|d| d.is_object()).ok_or_else(|| {
			Error::Invalid("invalid Provider Credential validation response".into())
		})?;
		let limit = data.get("limit").ok_or_else(|| {
			Error::Invalid("Provider Credential validation omitted spending limit".into())
		})?;
		if !limit.is_null() && !limit.is_number() {
			return Err(Error::Invalid(
				"invalid Provider Credential spending limit".into(),
			));
		}
		Ok(Validation {
			warnings: if limit.is_null() {
				vec!["No spending limit is configured for this Provider Credential".into()]
			} else {
				vec![]
			},
		})
	}
}
#[cfg(test)]
mod tests;
