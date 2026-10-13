//! Tenant-owned Provider Credential metadata and the closed Provider Catalog.
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Catalog IDs are portable; Provider Credential IDs never enter Registry configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
	Openrouter,
}
impl Provider {
	pub fn id(self) -> &'static str {
		"openrouter"
	}
	pub fn base_url(self) -> &'static str {
		"https://openrouter.ai/api/v1"
	}
	/// The provider-hosted consent page of Provider Authorization (OAuth PKCE).
	pub fn authorization_url(self) -> &'static str {
		"https://openrouter.ai/auth"
	}
	/// The owner-only key-settings page for one issued key, addressed by
	/// [`ProviderCredential::key_digest`].
	pub fn key_settings_url(self, key_digest: &str) -> String {
		format!("https://openrouter.ai/keys/{key_digest}")
	}
	pub fn parse(value: &str) -> Result<Self> {
		match value {
			"openrouter" => Ok(Self::Openrouter),
			_ => Err(Error::Invalid("unknown Provider Catalog entry".into())),
		}
	}
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum State {
	Pending,
	Active,
	Revoked,
	Deleted,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCredential {
	pub id: Uuid,
	pub tenant: String,
	pub provider: Provider,
	pub base_url: String,
	/// Secret references are adapter metadata, never included in API projections.
	pub secret_resource: String,
	pub pinned_version: Option<String>,
	pub fingerprint: String,
	pub last4: String,
	/// Unkeyed SHA-256 (hex) of the current Key Material, recorded only by
	/// Provider Authorization for the provider's key-settings deep link. This is
	/// ADR 0006's deliberate exception to the Fingerprint's no-correlation rule:
	/// every authorization issues a fresh random key.
	#[serde(default)]
	pub key_digest: Option<String>,
	pub state: State,
	pub created_at: DateTime<Utc>,
	pub rotated_at: Option<DateTime<Utc>>,
	pub revoked_at: Option<DateTime<Utc>>,
	pub revision: i64,
}
impl ProviderCredential {
	pub fn check_revision(&self, expected: i64) -> Result<()> {
		if self.revision != expected {
			return Err(Error::Conflict("stale Provider Credential revision".into()));
		}
		Ok(())
	}
	pub fn require_active(&self) -> Result<&str> {
		if self.state != State::Active {
			return Err(Error::Invalid("Provider Credential is not active".into()));
		}
		self.pinned_version
			.as_deref()
			.ok_or_else(|| Error::Invalid("Provider Credential version is unavailable".into()))
	}
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ProviderCredentialBinding")]
pub struct Binding {
	pub tenant: String,
	pub provider: Provider,
	pub provider_credential_id: Option<Uuid>,
	pub revision: i64,
}
/// Structural validation applies at every configuration write and on consumption.
pub fn validate_model_id(model: &str) -> Result<()> {
	if model.is_empty()
		|| model.len() > 256
		|| model.split('/').any(|part| {
			part.is_empty()
				|| part == "."
				|| part == ".."
				|| !part
					.bytes()
					.all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
		}) {
		return Err(Error::Invalid(
			"Provider Credential model ID must use catalog path segments and be at most 256 bytes"
				.into(),
		));
	}
	Ok(())
}

/// Structural validation applies at every configuration write and on consumption.
/// A configuration either names a Provider Catalog entry, whose exact provider
/// and base URL it must use, or calls its endpoint without a key.
pub fn validate_source(endpoint: &str, provider: &str, catalog: Option<&str>) -> Result<()> {
	crate::configuration::validate_endpoint(endpoint)?;
	if let Some(id) = catalog {
		let entry = Provider::parse(id)?;
		if provider != entry.id() || endpoint != entry.base_url() {
			return Err(Error::Invalid(
				"provider_credential requires its exact catalog provider and base URL".into(),
			));
		}
	}
	Ok(())
}
#[cfg(test)]
mod tests;
