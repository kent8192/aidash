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
	/// Minimum Data Protection Level for every Tenant on this Deployment.
	#[setting(default = "aidash_domain::data_protection::Level::Standard", leaf)]
	pub data_protection_floor: aidash_domain::data_protection::Level,
	/// Origins of models hosted by this Deployment, not merely network-permitted.
	#[setting(default = "Vec::new()", leaf)]
	pub hosted_model_origins: Vec<String>,
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
	/// Whether this Deployment federates with other Deployments. Off, it
	/// refuses every token-bearing request and sends none, and still publishes
	/// its key. Exchange between Nodes of this Deployment never depends on it.
	#[setting(default = "true")]
	pub federation: bool,
	/// `[deployment.key]`: where the Deployment Key lives. Without it this
	/// Deployment cannot federate with other Deployments.
	#[setting(optional, node)]
	pub key: Option<DeploymentKeySettings>,
}

/// `[deployment.key]`. Name exactly one of `file` and `kms`.
#[settings(fragment = true)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeploymentKeySettings {
	/// An Ed25519 private key in a PKCS#8 PEM file that only the server's user
	/// can read. A relative path starts at `core.base_dir`.
	#[setting(default = "None")]
	pub file: Option<String>,
	/// A Cloud KMS CryptoKeyVersion, `projects/.../cryptoKeyVersions/<n>`. Only
	/// the Enterprise build holds keys there.
	#[setting(default = "None")]
	pub kms: Option<String>,
}

/// Where the Deployment Key is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentKeySource<'a> {
	File(&'a str),
	Kms(&'a str),
}

impl DeploymentKeySettings {
	pub fn source(&self) -> Result<DeploymentKeySource<'_>, String> {
		match (
			self.file.as_deref().map(str::trim),
			self.kms.as_deref().map(str::trim),
		) {
			(Some(_), Some(_)) => Err("deployment.key names both file and kms; name one".into()),
			(Some(""), None) | (None, Some("")) => {
				Err("deployment.key.file and deployment.key.kms must not be blank".into())
			}
			(Some(file), None) => Ok(DeploymentKeySource::File(file)),
			(None, Some(kms)) => Ok(DeploymentKeySource::Kms(kms)),
			(None, None) => Err("deployment.key must name a file or a kms key version".into()),
		}
	}
}

impl fmt::Debug for DeploymentSettings {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("DeploymentSettings")
			.field("offering", &self.offering)
			.field("id", &self.id)
			.field("endpoint", &self.endpoint)
			.field("worker_count", &self.worker_count)
			.field("background_enabled", &self.background_enabled)
			.field("federation", &self.federation)
			.field("key", &self.key)
			.finish_non_exhaustive()
	}
}

impl SettingsValidation for DeploymentSettings {
	fn validate(&self, _profile: &Profile) -> ValidationResult {
		ValidateRules::validate(self)
			.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		self.hosted_origins()
			.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		DeploymentId::parse(&self.id)
			.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		validate_endpoint(&self.endpoint)
			.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		if let Some(key) = &self.key {
			key.source().map_err(ValidationError::Constraint)?;
		}
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

impl DeploymentSettings {
	pub fn hosted_origins(&self) -> crate::Result<aidash_domain::keyless::PermittedOrigins> {
		Ok(
			aidash_domain::keyless::PermittedOrigins::parse(&self.hosted_model_origins)
				.map_err(aidash_application::Error::from)?,
		)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	fn settings(extra: serde_json::Value) -> DeploymentSettings {
		let mut value = json!({
			"id": "aidash://test", "endpoint": "http://localhost",
			"api_token": "0123456789abcdef"
		});
		value
			.as_object_mut()
			.unwrap()
			.extend(extra.as_object().unwrap().clone());
		serde_json::from_value(value).unwrap()
	}

	fn check(settings: &DeploymentSettings) -> ValidationResult {
		SettingsValidation::validate(settings, &Profile::parse("local"))
	}

	fn problem(extra: serde_json::Value) -> String {
		check(&settings(extra)).unwrap_err().to_string()
	}

	#[test]
	fn federation_is_on_and_there_is_no_key_unless_the_settings_say_otherwise() {
		let defaults = settings(json!({}));
		assert!(defaults.federation);
		assert!(defaults.key.is_none());
		check(&defaults).unwrap();
		assert!(!settings(json!({"federation": false})).federation);
	}

	#[test]
	fn a_key_names_exactly_one_of_a_file_and_a_kms_version() {
		let file = settings(json!({"key": {"file": "keys/deployment.pem"}}));
		check(&file).unwrap();
		assert_eq!(
			file.key.as_ref().unwrap().source().unwrap(),
			DeploymentKeySource::File("keys/deployment.pem")
		);
		let kms = settings(json!({"key": {"kms": "projects/p/cryptoKeyVersions/1"}}));
		check(&kms).unwrap();
		assert_eq!(
			kms.key.as_ref().unwrap().source().unwrap(),
			DeploymentKeySource::Kms("projects/p/cryptoKeyVersions/1")
		);
		assert!(problem(json!({"key": {"file": "a", "kms": "b"}})).contains("both file and kms"));
		assert!(problem(json!({"key": {}})).contains("must name a file or a kms"));
		assert!(problem(json!({"key": {"file": "  "}})).contains("must not be blank"));
	}
}
