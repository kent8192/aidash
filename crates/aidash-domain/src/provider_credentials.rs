//! Tenant-owned Provider Credential metadata and the closed Provider Catalog.
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The one kind of grant that serves a Provider Catalog entry. A Catalog entry
/// never falls back from one kind to the other (ADR 0007).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
	/// A Tenant's Provider Credential, chosen by its Provider Credential Binding.
	ProviderCredential,
	/// The Plan Connection of the subject at the root of the Run's delegation chain.
	PlanConnection,
}

/// Catalog IDs are portable; Provider Credential and Plan Connection IDs never
/// enter Registry configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
	Openrouter,
	/// OpenAI's Responses API billed to a subject's ChatGPT plan (#159).
	Chatgpt,
}
impl Provider {
	pub fn id(self) -> &'static str {
		match self {
			Self::Openrouter => "openrouter",
			Self::Chatgpt => "chatgpt",
		}
	}
	pub fn base_url(self) -> &'static str {
		match self {
			Self::Openrouter => "https://openrouter.ai/api/v1",
			Self::Chatgpt => "https://api.openai.com/v1",
		}
	}
	pub fn grant(self) -> Grant {
		match self {
			Self::Openrouter => Grant::ProviderCredential,
			Self::Chatgpt => Grant::PlanConnection,
		}
	}
	/// The provider-hosted consent page: Provider Authorization (OAuth PKCE)
	/// for Provider Credentials, Sign in with ChatGPT for Plan Connections.
	pub fn authorization_url(self) -> &'static str {
		match self {
			Self::Openrouter => "https://openrouter.ai/auth",
			Self::Chatgpt => "https://auth.openai.com/api/accounts/authorize",
		}
	}
	/// The owner-only key-settings page for one issued key, addressed by
	/// [`ProviderCredential::key_digest`]. Plan Connections hold no key.
	pub fn key_settings_url(self, key_digest: &str) -> Option<String> {
		match self {
			Self::Openrouter => Some(format!("https://openrouter.ai/keys/{key_digest}")),
			Self::Chatgpt => None,
		}
	}
	/// Any Provider Catalog entry.
	pub fn parse(value: &str) -> Result<Self> {
		match value {
			"openrouter" => Ok(Self::Openrouter),
			"chatgpt" => Ok(Self::Chatgpt),
			_ => Err(Error::Invalid("unknown Provider Catalog entry".into())),
		}
	}
	/// A Provider Catalog entry served by Provider Credentials.
	pub fn credential(value: &str) -> Result<Self> {
		Self::parse(value)?.require_credential()
	}
	/// Refuses an entry that Provider Credentials do not serve, at each
	/// boundary that receives an already-parsed [`Provider`].
	pub fn require_credential(self) -> Result<Self> {
		self.require(Grant::ProviderCredential)
	}
	/// A Provider Catalog entry served by Plan Connections.
	pub fn plan(value: &str) -> Result<Self> {
		Self::parse(value)?.require(Grant::PlanConnection)
	}
	fn require(self, grant: Grant) -> Result<Self> {
		if self.grant() != grant {
			return Err(Error::Invalid(match grant {
				Grant::ProviderCredential => {
					"Provider Catalog entry is not served by Provider Credentials".into()
				}
				Grant::PlanConnection => {
					"Provider Catalog entry is not served by Plan Connections".into()
				}
			}));
		}
		Ok(self)
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
/// A configuration either names a Provider Catalog entry served by Provider
/// Credentials, whose exact provider and base URL it must use, or calls its
/// endpoint without a key.
pub fn validate_source(endpoint: &str, provider: &str, catalog: Option<&str>) -> Result<()> {
	crate::configuration::validate_endpoint(endpoint)?;
	if let Some(id) = catalog {
		let entry = Provider::credential(id)?;
		if provider != entry.id() || endpoint != entry.base_url() {
			return Err(Error::Invalid(
				"provider_credential requires its exact catalog provider and base URL".into(),
			));
		}
	}
	Ok(())
}

/// Model configurations name at most one grant: a Provider Credential entry, a
/// Plan Connection entry, or neither for a keyless endpoint. A Plan Connection
/// entry fixes the provider and base URL exactly like a Provider Credential one.
pub fn validate_grants(
	endpoint: &str,
	provider: &str,
	provider_credential: Option<&str>,
	plan_connection: Option<&str>,
) -> Result<()> {
	let Some(id) = plan_connection else {
		return validate_source(endpoint, provider, provider_credential);
	};
	if provider_credential.is_some() {
		return Err(Error::Invalid(
			"a model names either provider_credential or plan_connection, never both".into(),
		));
	}
	crate::configuration::validate_endpoint(endpoint)?;
	let entry = Provider::plan(id)?;
	if provider != entry.id() || endpoint != entry.base_url() {
		return Err(Error::Invalid(
			"plan_connection requires its exact catalog provider and base URL".into(),
		));
	}
	Ok(())
}
#[cfg(test)]
mod tests;
