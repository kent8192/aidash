//! The Deployment's permitted private origins for Keyless endpoints (ADR 0036).
use aidash_domain::keyless::PermittedOrigins;
use reinhardt::{
	conf::settings::{
		fragment::SettingsValidation,
		profile::Profile,
		validation::{ValidationError, ValidationResult},
	},
	settings,
};

/// `[keyless_endpoints]` with `permitted_private_origins = ["http://ollama.lan:11434"]`.
/// A Tenant's Keyless endpoint may connect only to public addresses unless its
/// exact origin is listed here. Origins are normalized, so a default port is
/// omitted: list `http://ollama.lan`, not `http://ollama.lan:80`.
#[settings(fragment = true, section = "keyless_endpoints", validate = false)]
pub struct KeylessEndpointSettings {
	#[setting(default = "Vec::new()", leaf)]
	pub permitted_private_origins: Vec<String>,
}

impl KeylessEndpointSettings {
	pub fn origins(&self) -> crate::Result<PermittedOrigins> {
		Ok(PermittedOrigins::parse(&self.permitted_private_origins)
			.map_err(aidash_application::Error::from)?)
	}
}

impl SettingsValidation for KeylessEndpointSettings {
	fn validate(&self, _: &Profile) -> ValidationResult {
		self.origins()
			.map(drop)
			.map_err(|error| ValidationError::Constraint(error.to_string()))
	}
}
