//! Optional dashboard OIDC configuration, composed with native settings.
use crate::{Error, Result};
use reinhardt::conf::settings::fragment::SettingsValidation;
use reinhardt::conf::settings::profile::Profile;
use reinhardt::conf::settings::validation::{ValidationError, ValidationResult};
use reinhardt::core::validators::Validate as ValidateRules;
use reinhardt::{Validate, settings};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::fmt;

#[settings(fragment = true, section = "dashboard", validate = false)]
pub struct DashboardSettings {
	#[setting(optional, node)]
	pub oidc: Option<OidcConfig>,
	#[setting(optional, node)]
	pub gcip: Option<GcipConfig>,
}

#[settings(fragment = true)]
#[derive(Clone, Serialize, Deserialize, Validate, schemars::JsonSchema)]
pub struct OidcConfig {
	#[setting(default = "\"https://accounts.google.com\".to_string()")]
	#[validate(length(min = 1))]
	pub issuer: String,
	#[setting(required)]
	#[validate(length(min = 1))]
	pub client_id: String,
	#[setting(required)]
	#[validate(length(min = 1))]
	pub client_secret: String,
	#[setting(required)]
	pub public_origin: String,
	#[setting(default = "String::new()")]
	pub keycloak_admin_url: String,
	#[setting(default = "String::new()")]
	pub status_client_id: String,
	#[setting(default = "String::new()")]
	pub status_client_secret: String,
	#[setting(default = "43200")]
	#[validate(range(min = 60, max = 604800))]
	pub session_absolute_seconds: i64,
	#[setting(default = "1800")]
	#[validate(range(min = 60, max = 604800))]
	pub session_idle_seconds: i64,
}

impl fmt::Debug for OidcConfig {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("OidcConfig")
			.field("issuer", &self.issuer)
			.field("client_id", &self.client_id)
			.field("public_origin", &self.public_origin)
			.finish_non_exhaustive()
	}
}

impl OidcConfig {
	pub fn is_google(&self) -> bool {
		self.issuer == crate::config::GOOGLE_OIDC_ISSUER
	}
	pub fn normalized(&self) -> Result<Self> {
		for (name, value) in [
			("client_id", &self.client_id),
			("client_secret", &self.client_secret),
		] {
			if value.trim().is_empty() {
				return Err(Error::Invalid(format!(
					"dashboard.oidc.{name} must not be blank"
				)));
			}
		}
		ValidateRules::validate(self).map_err(|error| Error::Invalid(error.to_string()))?;
		for (name, value) in [
			("issuer", &self.issuer),
			("public_origin", &self.public_origin),
			("keycloak_admin_url", &self.keycloak_admin_url),
		] {
			if name == "keycloak_admin_url" && self.is_google() {
				continue;
			}
			let url = Url::parse(value)
				.map_err(|_| Error::Invalid(format!("invalid dashboard.oidc.{name}")))?;
			let secure = url.scheme() == "https";
			let local =
				url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
			if (!secure && !local)
				|| url.host_str().is_none()
				|| !url.username().is_empty()
				|| url.password().is_some()
				|| url.query().is_some()
				|| url.fragment().is_some()
			{
				return Err(Error::Invalid(format!("invalid dashboard.oidc.{name}")));
			}
		}
		if !self.is_google()
			&& (self.status_client_id.trim().is_empty()
				|| self.status_client_secret.trim().is_empty())
		{
			return Err(Error::Invalid(
				"OIDC status credentials are required for Keycloak".into(),
			));
		}
		let origin = Url::parse(&self.public_origin)
			.map_err(|_| Error::Invalid("invalid dashboard.oidc.public_origin".into()))?;
		if origin.path() != "/" {
			return Err(Error::Invalid(
				"dashboard.oidc.public_origin must not contain a path".into(),
			));
		}
		if self.session_idle_seconds > self.session_absolute_seconds {
			return Err(Error::Invalid(
				"OIDC session idle time cannot exceed its absolute lifetime".into(),
			));
		}
		Ok(Self {
			public_origin: origin.origin().ascii_serialization(),
			keycloak_admin_url: self.keycloak_admin_url.trim_end_matches('/').into(),
			..self.clone()
		})
	}
}

#[settings(fragment = true)]
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GcipConfig {
	#[setting(required)]
	pub project_id: String,
	#[setting(required)]
	pub web_api_key: String,
	#[setting(required)]
	pub public_origin: String,
	#[setting(required)]
	pub tenant_bindings: std::collections::BTreeMap<String, String>,
	#[setting(default = "Default::default()")]
	pub providers: std::collections::BTreeMap<String, Vec<String>>,
	#[setting(default = "Default::default()")]
	#[setting(leaf)]
	pub password_sign_up: std::collections::BTreeSet<String>,
	/// Sign-in Domain → GCIP Tenant ID. Routing only; never authority.
	#[setting(default = "Default::default()")]
	pub sign_in_domains: std::collections::BTreeMap<String, String>,
	#[setting(default = "43200")]
	pub session_absolute_seconds: i64,
	#[setting(default = "1800")]
	pub session_idle_seconds: i64,
}
impl fmt::Debug for GcipConfig {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("GcipConfig")
			.field("project_id", &self.project_id)
			.field("public_origin", &self.public_origin)
			.field("tenant_bindings", &self.tenant_bindings)
			.finish_non_exhaustive()
	}
}
impl GcipConfig {
	pub fn issuer(&self) -> String {
		format!("https://securetoken.google.com/{}", self.project_id)
	}
	pub fn normalized(&self) -> Result<Self> {
		if self.project_id.is_empty()
			|| self.project_id.len() > 63
			|| !self
				.project_id
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b == b'-')
			|| self.web_api_key.trim().is_empty()
		{
			return Err(Error::Invalid(
				"dashboard.gcip requires a project ID and web API key".into(),
			));
		}
		let origin = Url::parse(&self.public_origin)
			.map_err(|_| Error::Invalid("invalid dashboard.gcip.public_origin".into()))?;
		if !(origin.scheme() == "https"
			|| (origin.scheme() == "http"
				&& matches!(origin.host_str(), Some("localhost" | "127.0.0.1"))))
			|| origin.host_str().is_none()
			|| !origin.username().is_empty()
			|| origin.password().is_some()
			|| origin.query().is_some()
			|| origin.fragment().is_some()
			|| origin.path() != "/"
		{
			return Err(Error::Invalid(
				"invalid dashboard.gcip.public_origin".into(),
			));
		}
		let mut tenants = std::collections::BTreeSet::new();
		for (pool, tenant) in &self.tenant_bindings {
			if pool.is_empty()
				|| pool.len() > 128
				|| !pool.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
			{
				return Err(Error::Invalid("invalid GCIP Tenant ID".into()));
			}
			aidash_domain::policy::identifier(tenant)?;
			if !tenants.insert(tenant) {
				return Err(Error::Invalid("Tenant Bindings must be one-to-one".into()));
			}
		}
		if self
			.providers
			.keys()
			.chain(self.password_sign_up.iter())
			.any(|pool| !self.tenant_bindings.contains_key(pool))
		{
			return Err(Error::Invalid(
				"GCIP provider configuration requires a Tenant Binding".into(),
			));
		}
		if self.providers.values().flatten().any(|id| {
			id != "password"
				&& id != "google.com"
				&& !id.starts_with("saml.")
				&& !id.starts_with("oidc.")
		}) {
			return Err(Error::Invalid("unsupported GCIP sign-in provider".into()));
		}
		// Runtime lookups use the configured keys, so they must already be in the
		// canonical form that addresses are normalized to (lowercase IDNA ASCII).
		for (domain, pool) in &self.sign_in_domains {
			if aidash_domain::identity::dashboard::sign_in_domain(domain).as_ref() != Some(domain) {
				return Err(Error::Invalid(
					"GCIP Sign-in Domains must be lowercase ASCII DNS names".into(),
				));
			}
			if !self.tenant_bindings.contains_key(pool) {
				return Err(Error::Invalid(
					"GCIP Sign-in Domain requires a Tenant Binding".into(),
				));
			}
		}
		if !(60..=604800).contains(&self.session_idle_seconds)
			|| !(60..=604800).contains(&self.session_absolute_seconds)
			|| self.session_idle_seconds > self.session_absolute_seconds
		{
			return Err(Error::Invalid("invalid GCIP session lifetime".into()));
		}
		Ok(Self {
			public_origin: origin.origin().ascii_serialization(),
			..self.clone()
		})
	}
}

/// Cookie, CSRF and desktop policy is independent of the authentication adapter.
#[derive(Clone, Copy)]
pub struct SessionConfig<'a> {
	pub public_origin: &'a str,
	pub session_absolute_seconds: i64,
	pub session_idle_seconds: i64,
}
impl<'a> From<&'a OidcConfig> for SessionConfig<'a> {
	fn from(config: &'a OidcConfig) -> Self {
		Self {
			public_origin: &config.public_origin,
			session_absolute_seconds: config.session_absolute_seconds,
			session_idle_seconds: config.session_idle_seconds,
		}
	}
}
impl<'a> From<&'a GcipConfig> for SessionConfig<'a> {
	fn from(config: &'a GcipConfig) -> Self {
		Self {
			public_origin: &config.public_origin,
			session_absolute_seconds: config.session_absolute_seconds,
			session_idle_seconds: config.session_idle_seconds,
		}
	}
}

impl SettingsValidation for DashboardSettings {
	fn validate(&self, _profile: &Profile) -> ValidationResult {
		if self.oidc.is_some() && self.gcip.is_some() {
			return Err(ValidationError::Constraint(
				"dashboard.oidc and dashboard.gcip are mutually exclusive".into(),
			));
		}
		if let Some(config) = &self.gcip {
			config
				.normalized()
				.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		}
		if let Some(config) = &self.oidc {
			config
				.normalized()
				.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::{fixture, rstest};

	#[fixture]
	fn oidc_config() -> OidcConfig {
		serde_json::from_value(serde_json::json!({
			"client_id": "fixture-client",
			"client_secret": "fixture-secret",
			"public_origin": "https://dashboard.example.test",
		}))
		.unwrap()
	}

	#[rstest]
	#[case::empty_id("", "fixture-secret", "client_id")]
	#[case::spaces_id("   ", "fixture-secret", "client_id")]
	#[case::tabs_id("\t\n", "fixture-secret", "client_id")]
	#[case::empty_secret("fixture-client", "", "client_secret")]
	#[case::spaces_secret("fixture-client", "   ", "client_secret")]
	#[case::tabs_secret("fixture-client", "\t\n", "client_secret")]
	fn blank_oidc_credentials_fail_startup_validation(
		mut oidc_config: OidcConfig,
		#[case] client_id: &str,
		#[case] client_secret: &str,
		#[case] field: &str,
	) {
		// Arrange
		oidc_config.client_id = client_id.into();
		oidc_config.client_secret = client_secret.into();
		let dashboard = DashboardSettings {
			oidc: Some(oidc_config),
			gcip: None,
		};
		// Act
		let error = dashboard.validate(&Profile::Development).unwrap_err();
		// Assert: fail before any provider request without exposing credentials.
		assert!(
			error
				.to_string()
				.contains(&format!("dashboard.oidc.{field} must not be blank"))
		);
		assert!(!error.to_string().contains("fixture-secret"));
	}

	#[rstest]
	fn nonblank_credentials_retain_their_original_bytes(mut oidc_config: OidcConfig) {
		// Arrange
		oidc_config.client_id = " fixture-client ".into();
		oidc_config.client_secret = " fixture-secret ".into();
		// Act
		let normalized = oidc_config.normalized().unwrap();
		// Assert: validation must not trim significant credential bytes.
		assert_eq!(normalized.client_id, oidc_config.client_id);
		assert_eq!(normalized.client_secret, oidc_config.client_secret);
	}
	#[fixture]
	fn gcip_config() -> GcipConfig {
		serde_json::from_value(serde_json::json!({"project_id":"fixture-project","web_api_key":"public-key","public_origin":"https://dashboard.example.test/","tenant_bindings":{"pool-a":"acme"}})).unwrap()
	}
	#[rstest]
	fn gcip_and_oidc_are_mutually_exclusive(oidc_config: OidcConfig, gcip_config: GcipConfig) {
		assert!(
			DashboardSettings {
				oidc: Some(oidc_config),
				gcip: Some(gcip_config)
			}
			.validate(&Profile::Development)
			.is_err()
		);
	}
	#[rstest]
	fn gcip_bindings_are_one_to_one_and_can_be_removed(mut gcip_config: GcipConfig) {
		assert_eq!(
			gcip_config.normalized().unwrap().public_origin,
			"https://dashboard.example.test"
		);
		gcip_config
			.tenant_bindings
			.insert("pool-b".into(), "acme".into());
		assert!(gcip_config.normalized().is_err());
		gcip_config.tenant_bindings.clear();
		assert!(gcip_config.normalized().is_ok());
	}
	#[rstest]
	#[case("http://dashboard.example.test")]
	#[case("https://dashboard.example.test/path")]
	#[case("https://user:password@dashboard.example.test")]
	fn gcip_rejects_invalid_origins(mut gcip_config: GcipConfig, #[case] origin: &str) {
		gcip_config.public_origin = origin.into();
		assert!(gcip_config.normalized().is_err());
	}
	#[rstest]
	#[case("acme.com", "pool-b", "GCIP Sign-in Domain requires a Tenant Binding")]
	#[case(
		"ACME.com",
		"pool-a",
		"GCIP Sign-in Domains must be lowercase ASCII DNS names"
	)]
	#[case(
		"bücher.example",
		"pool-a",
		"GCIP Sign-in Domains must be lowercase ASCII DNS names"
	)]
	#[case(
		"localhost",
		"pool-a",
		"GCIP Sign-in Domains must be lowercase ASCII DNS names"
	)]
	fn gcip_rejects_noncanonical_or_unbound_sign_in_domains(
		mut gcip_config: GcipConfig,
		#[case] domain: &str,
		#[case] pool: &str,
		#[case] reason: &str,
	) {
		gcip_config
			.sign_in_domains
			.insert(domain.into(), pool.into());
		assert!(
			matches!(gcip_config.normalized(), Err(Error::Invalid(message)) if message == reason)
		);
		gcip_config.sign_in_domains = [("xn--bcher-kva.example".into(), "pool-a".into())].into();
		assert!(gcip_config.normalized().is_ok());
	}
}
