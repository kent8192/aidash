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
	pub common_auth: Option<CommonAuthSettings>,
}

/// Explicit Cloud-only common authentication configuration. Dedicated and
/// Self-hosted keep their independently configured sign-in provider.
#[settings(fragment = true, validate = false)]
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CommonAuthSettings {
	#[setting(required)]
	pub grpc_endpoint: String,
	#[setting(required)]
	pub server_domain: String,
	#[setting(required)]
	pub ca_certificate_file: std::path::PathBuf,
	#[setting(required)]
	pub client_certificate_file: std::path::PathBuf,
	#[setting(required)]
	pub client_private_key_file: std::path::PathBuf,
	/// The original upstream issuers, including any GCIP pool suffix. These
	/// restrictions govern product-local Mapping selection after OIDC login.
	/// Empty value means unrestricted; otherwise the value is the bound Tenant.
	#[setting(required)]
	pub source_issuers: std::collections::BTreeMap<String, String>,
}

impl CommonAuthSettings {
	pub fn client(&self, client_id: &str) -> Result<aidash_integrations::authentication::Client> {
		if self.source_issuers.is_empty()
			|| self.source_issuers.keys().any(|issuer| issuer.is_empty())
		{
			return Err(Error::Invalid(
				"dashboard.common_auth.source_issuers is required".into(),
			));
		}
		for tenant in self
			.source_issuers
			.values()
			.filter(|tenant| !tenant.is_empty())
		{
			aidash_domain::tenancy::TenantName::new(tenant.clone())?;
		}
		let read = |path: &std::path::Path| {
			std::fs::read(path).map_err(|_| {
				Error::Invalid("common authentication TLS material cannot be read".into())
			})
		};
		aidash_integrations::authentication::Client::new(
			aidash_integrations::authentication::Settings {
				endpoint: self.grpc_endpoint.clone(),
				server_domain: self.server_domain.clone(),
				ca_certificate: read(&self.ca_certificate_file)?,
				client_certificate: read(&self.client_certificate_file)?,
				client_private_key: read(&self.client_private_key_file)?,
				product_client_id: client_id.to_owned(),
			},
		)
		.map_err(Into::into)
	}
}

impl aidash_application::ports::authorization::dashboard::Issuers for CommonAuthSettings {
	fn admitted(&self) -> Vec<String> {
		self.source_issuers.keys().cloned().collect()
	}
	fn restriction(&self, issuer: &str) -> Option<String> {
		self.source_issuers
			.get(issuer)
			.filter(|tenant| !tenant.is_empty())
			.cloned()
	}
	fn restricted_issuer(&self, tenant: &str) -> Option<String> {
		self.source_issuers
			.iter()
			.find_map(|(issuer, restriction)| (restriction == tenant).then(|| issuer.clone()))
	}
	fn retires_refused(&self) -> bool {
		true
	}
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
	#[setting(default = "2592000")]
	#[validate(range(min = 60, max = 2592000))]
	pub session_absolute_seconds: i64,
	#[setting(default = "604800")]
	#[validate(range(min = 60, max = 2592000))]
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
		] {
			let url = Url::parse(value)
				.map_err(|_| Error::Invalid(format!("invalid dashboard.oidc.{name}")))?;
			let secure = url.scheme() == "https";
			let local_issuer = name == "issuer"
				&& url.scheme() == "http"
				&& matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
			if (!secure && !local_issuer)
				|| url.host_str().is_none()
				|| !url.username().is_empty()
				|| url.password().is_some()
				|| url.query().is_some()
				|| url.fragment().is_some()
			{
				return Err(Error::Invalid(format!("invalid dashboard.oidc.{name}")));
			}
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
impl SettingsValidation for DashboardSettings {
	fn validate(&self, _profile: &Profile) -> ValidationResult {
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
			common_auth: None,
			oidc: Some(oidc_config),
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

	#[rstest]
	#[case::ipv4("http://127.0.0.1:8080")]
	#[case::hostname("http://localhost:8080")]
	fn loopback_http_public_origins_fail_startup_validation(
		mut oidc_config: OidcConfig,
		#[case] public_origin: &str,
	) {
		// Arrange
		oidc_config.public_origin = public_origin.into();
		let dashboard = DashboardSettings {
			common_auth: None,
			oidc: Some(oidc_config),
		};
		// Act
		let error = dashboard.validate(&Profile::Development).unwrap_err();
		// Assert: browser credentials must never be issued on a host-wide HTTP origin.
		assert!(
			error
				.to_string()
				.contains("invalid dashboard.oidc.public_origin")
		);
	}

	#[rstest]
	fn browser_sessions_default_to_seven_days_idle_and_thirty_days(oidc_config: OidcConfig) {
		// Assert: #627 keeps people signed in within NIST SP 800-63B-4 AAL1 limits.
		assert_eq!(oidc_config.session_idle_seconds, 604_800);
		assert_eq!(oidc_config.session_absolute_seconds, 2_592_000);
	}

	#[rstest]
	#[case::at_bound(2_592_000, 2_592_000, true)]
	#[case::absolute_over(604_800, 2_592_001, false)]
	#[case::idle_over(2_592_001, 2_592_000, false)]
	#[case::idle_beyond_absolute(604_801, 604_800, false)]
	fn session_lifetimes_stay_within_thirty_days(
		mut oidc_config: OidcConfig,
		#[case] idle: i64,
		#[case] absolute: i64,
		#[case] accepted: bool,
	) {
		// Arrange
		oidc_config.session_idle_seconds = idle;
		oidc_config.session_absolute_seconds = absolute;
		// Act
		let result = oidc_config.normalized();
		// Assert
		assert_eq!(result.is_ok(), accepted);
	}

	#[rstest]
	fn https_public_origin_and_loopback_http_issuer_remain_supported(mut oidc_config: OidcConfig) {
		// Arrange
		oidc_config.issuer = "http://127.0.0.1:18099/issuer".into();
		oidc_config.public_origin = "https://dashboard.example.test:8443/".into();
		// Act
		let normalized = oidc_config.normalized().unwrap();
		// Assert
		assert_eq!(normalized.issuer, oidc_config.issuer);
		assert_eq!(
			normalized.public_origin,
			"https://dashboard.example.test:8443"
		);
	}
}
