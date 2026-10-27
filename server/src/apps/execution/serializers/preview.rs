//! `[preview]`: whether this Deployment serves Previews, and where (ADR 0068).
use reinhardt::conf::settings::{
	fragment::SettingsValidation,
	profile::Profile,
	validation::{ValidationError, ValidationResult},
};
use reinhardt::settings;
use std::net::SocketAddr;

/// The preview domain is optional. Without one, the dashboard reports that
/// Previews are unavailable and no HTML/PDF File preview opens.
#[settings(fragment = true, section = "preview", validate = false)]
pub struct PreviewSettings {
	/// A registrable domain separate from the dashboard's; a local cluster uses
	/// `localhost` and previews under `*.localhost`. Omitting every field makes
	/// Previews unavailable.
	#[setting(default = "None")]
	pub domain: Option<String>,
	/// Where the gateway listens. A Self-hosted Deployment that runs the gateway
	/// in-process binds loopback; a separate Deployment binds its Pod address.
	#[setting(default = "None", leaf)]
	pub listen: Option<SocketAddr>,
	/// Whether the API process also runs the gateway. Helm turns this off when
	/// it deploys the gateway as its own workload.
	// A default here also lets a Deployment omit the whole `[preview]` section.
	#[setting(default = "true")]
	pub in_process: bool,
	/// The public port a ticket URL names; 443 when absent.
	#[setting(default = "None")]
	pub public_port: Option<u16>,
}

/// The resolved Preview configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewConfig {
	pub domain: String,
	pub listen: SocketAddr,
	pub in_process: bool,
	pub public_port: Option<u16>,
}

/// The gateway's loopback listener when the settings name none.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8960";

impl PreviewSettings {
	/// Resolve and validate the settings against the dashboard's public origin.
	/// A Deployment refuses to start when the two share a registrable domain.
	pub fn configured(&self, dashboard_origin: &str) -> crate::Result<Option<PreviewConfig>> {
		let Some(domain) = self.domain.as_deref() else {
			return Ok(None);
		};
		let domain = aidash_domain::preview::preview_domain(domain).map_err(crate::Error::from)?;
		aidash_domain::preview::refuse_shared_registrable_domain(&domain, dashboard_origin)
			.map_err(crate::Error::from)?;
		let listen = self
			.listen
			.unwrap_or_else(|| DEFAULT_LISTEN.parse().expect("default preview listener"));
		if listen.port() == 0 {
			return Err(crate::Error::Invalid(
				"preview.listen must name a port".into(),
			));
		}
		if self.public_port == Some(0) {
			return Err(crate::Error::Invalid(
				"preview.public_port must not be zero".into(),
			));
		}
		Ok(Some(PreviewConfig {
			domain,
			listen,
			in_process: self.in_process,
			public_port: self.public_port,
		}))
	}
}

impl SettingsValidation for PreviewSettings {
	fn validate(&self, _: &Profile) -> ValidationResult {
		if let Some(domain) = &self.domain {
			aidash_domain::preview::preview_domain(domain)
				.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		}
		if let Some(listen) = self.listen
			&& listen.port() == 0
		{
			return Err(ValidationError::Constraint(
				"preview.listen must name a port".into(),
			));
		}
		if self.public_port == Some(0) {
			return Err(ValidationError::Constraint(
				"preview.public_port must not be zero".into(),
			));
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn settings(domain: Option<&str>) -> PreviewSettings {
		PreviewSettings {
			domain: domain.map(str::to_owned),
			listen: None,
			in_process: true,
			public_port: None,
		}
	}

	#[test]
	fn no_domain_means_no_previews() {
		assert_eq!(
			settings(None)
				.configured("https://dashboard.example")
				.unwrap(),
			None
		);
	}

	#[test]
	fn a_preview_domain_must_be_registrable_and_separate() {
		let configured = settings(Some("aidashusercontent.app"))
			.configured("https://app.example.com")
			.unwrap()
			.unwrap();
		assert_eq!(configured.domain, "aidashusercontent.app");
		assert_eq!(configured.listen, DEFAULT_LISTEN.parse().unwrap());
		assert!(configured.in_process);
		assert!(
			settings(Some("preview.example.com"))
				.configured("https://app.example.com")
				.is_err()
		);
	}

	#[test]
	fn a_local_preview_domain_uses_localhost() {
		let configured = settings(Some("localhost"))
			.configured("http://localhost:8080")
			.unwrap()
			.unwrap();
		assert_eq!(configured.domain, "localhost");
	}
}
