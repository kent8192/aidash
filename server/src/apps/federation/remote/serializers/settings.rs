//! Node identity, operator access, and background execution settings.
use crate::config::{validate_endpoint, validate_node_id};
use reinhardt::conf::settings::fragment::SettingsValidation;
use reinhardt::conf::settings::profile::Profile;
use reinhardt::conf::settings::validation::{ValidationError, ValidationResult};
use reinhardt::core::validators::Validate as ValidateRules;
use reinhardt::{Validate, settings};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::SocketAddr;

#[settings(fragment = true, section = "node", validate = false)]
#[derive(Clone, Serialize, Deserialize, Validate)]
pub struct NodeSettings {
	#[setting(required)]
	pub node_id: String,
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
	/// Pending operator packages for newly created tenants; never an approval.
	#[setting(default = "Vec::new()", leaf)]
	pub default_host_packages: Vec<String>,
	#[setting(default = "4")]
	#[validate(range(min = 0, max = 64))]
	pub worker_count: usize,
	#[setting(default = "true")]
	pub background_enabled: bool,
	#[setting(default = "None", leaf)]
	pub probe_listen: Option<SocketAddr>,
}

impl fmt::Debug for NodeSettings {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("NodeSettings")
			.field("node_id", &self.node_id)
			.field("endpoint", &self.endpoint)
			.field("worker_count", &self.worker_count)
			.field("background_enabled", &self.background_enabled)
			.finish_non_exhaustive()
	}
}

impl SettingsValidation for NodeSettings {
	fn validate(&self, _profile: &Profile) -> ValidationResult {
		ValidateRules::validate(self)
			.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		validate_node_id(&self.node_id)
			.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		validate_endpoint(&self.endpoint)
			.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		let mut names = std::collections::BTreeSet::new();
		for name in &self.default_host_packages {
			if !names.insert(name)
				|| !aidash_application::registry::system::packages::HOST_GROUPS
					.iter()
					.any(|(group, _)| name == group)
			{
				return Err(ValidationError::Constraint(
					"node.default_host_packages must contain unique known host groups".into(),
				));
			}
		}
		let url = reqwest::Url::parse(&self.nats_url)
			.map_err(|_| ValidationError::Constraint("node.nats_url must be a NATS URL".into()))?;
		if !matches!(url.scheme(), "nats" | "tls" | "ws" | "wss") || url.host_str().is_none() {
			return Err(ValidationError::Constraint(
				"node.nats_url must identify a NATS server".into(),
			));
		}
		Ok(())
	}
}
