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
}
