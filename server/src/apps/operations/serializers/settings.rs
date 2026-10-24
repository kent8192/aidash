//! Optional Kubernetes observation settings.
use crate::{apps::operations::services::core::label, config::validate_endpoint};
use reinhardt::conf::settings::{
	fragment::SettingsValidation,
	profile::Profile,
	validation::{ValidationError, ValidationResult},
};
use reinhardt::settings;

#[settings(fragment = true, section = "kubernetes", validate = false)]
pub struct KubernetesSettings {
	#[setting(default = "false")]
	pub enabled: bool,
	#[setting(optional)]
	pub namespace: String,
	#[setting(optional)]
	pub release: String,
	#[setting(default = "String::from(\"https://kubernetes.default.svc\")")]
	pub endpoint: String,
	#[setting(default = "String::from(\"/var/run/secrets/kubernetes.io/serviceaccount/ca.crt\")")]
	pub ca_file: String,
	#[setting(default = "String::from(\"/var/run/secrets/kubernetes.io/serviceaccount/token\")")]
	pub token_file: String,
}

impl SettingsValidation for KubernetesSettings {
	fn validate(&self, _profile: &Profile) -> ValidationResult {
		if !self.enabled {
			return Ok(());
		}
		if !label(&self.namespace) || !label(&self.release) {
			return Err(ValidationError::Constraint(
				"Kubernetes observations require valid namespace and release labels".into(),
			));
		}
		validate_endpoint(&self.endpoint)
			.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		if self.token_file.is_empty()
			|| (self.endpoint.starts_with("https:") && self.ca_file.is_empty())
		{
			return Err(ValidationError::Constraint(
				"Kubernetes token and HTTPS CA file paths must not be empty".into(),
			));
		}
		Ok(())
	}
}
