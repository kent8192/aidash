//! `[deployment]`: the identity and runtime settings of the whole Deployment.
use crate::config::validate_endpoint;
use aidash_domain::nodes::DeploymentId;
use aidash_domain::offering::Offering;
use reinhardt::conf::settings::fragment::SettingsValidation;
use reinhardt::conf::settings::profile::Profile;
use reinhardt::conf::settings::validation::{ValidationError, ValidationResult};
use reinhardt::core::validators::Validate as ValidateRules;
use reinhardt::{Validate, settings};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::SocketAddr;

#[settings(fragment = true, section = "deployment", validate = false)]
#[derive(Clone, Serialize, Deserialize, Validate)]
pub struct DeploymentSettings {
	/// The Offering this Deployment runs. Each build accepts only its own
	/// Offerings, so Cloud and Dedicated must state theirs.
	#[setting(default = "Offering::SelfHosted", leaf)]
	pub offering: Offering,
	/// The Deployment id, `aidash://<name>`.
	#[setting(required)]
	pub id: String,
	#[setting(required)]
	pub endpoint: String,
	#[setting(required)]
	#[validate(length(min = 16))]
	pub api_token: String,
	#[setting(default = "String::from(\"nats://127.0.0.1:4222\")")]
	pub nats_url: String,
	#[setting(default = "String::from(\"../web/dist\")")]
	pub web_dir: String,
	#[setting(default = "30")]
	#[validate(range(min = 3, max = 3600))]
	pub lease_seconds: i32,
	#[setting(default = "4")]
	#[validate(range(min = 0, max = 64))]
	pub worker_count: usize,
	#[setting(default = "true")]
	pub background_enabled: bool,
	#[setting(default = "None", leaf)]
	pub probe_listen: Option<SocketAddr>,
}

impl fmt::Debug for DeploymentSettings {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("DeploymentSettings")
			.field("offering", &self.offering)
			.field("id", &self.id)
			.field("endpoint", &self.endpoint)
			.field("worker_count", &self.worker_count)
			.field("background_enabled", &self.background_enabled)
			.finish_non_exhaustive()
	}
}

impl SettingsValidation for DeploymentSettings {
	fn validate(&self, _profile: &Profile) -> ValidationResult {
		ValidateRules::validate(self)
			.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		DeploymentId::parse(&self.id)
			.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		validate_endpoint(&self.endpoint)
			.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		let url = reqwest::Url::parse(&self.nats_url).map_err(|_| {
			ValidationError::Constraint("deployment.nats_url must be a NATS URL".into())
		})?;
		if !matches!(url.scheme(), "nats" | "tls" | "ws" | "wss") || url.host_str().is_none() {
			return Err(ValidationError::Constraint(
				"deployment.nats_url must identify a NATS server".into(),
			));
		}
		Ok(())
	}
}
