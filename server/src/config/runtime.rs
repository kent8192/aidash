use crate::{Error, Result};
use reinhardt::conf::settings::{fragment::SettingsValidation, profile::Profile};
use std::env;

#[derive(Clone)]
pub struct Config {
	pub node_id: String,
	pub endpoint: String,
	pub database_url: String,
	pub nats_url: String,
	pub api_token: String,
	pub web_dir: String,
	pub lease_seconds: i32,
	pub default_host_packages: Vec<String>,
	pub oidc: Option<OidcConfig>,
}

pub use crate::apps::identity::serializers::settings::OidcConfig;

pub const PROTOCOL_VERSION: &str = "0.1";

impl Config {
	pub fn from_settings(settings: &super::settings::ProjectSettings) -> Result<Self> {
		settings
			.node
			.validate(&Profile::parse("local"))
			.map_err(|error| Error::Invalid(error.to_string()))?;
		let database = settings
			.core
			.databases
			.get("default")
			.ok_or_else(|| Error::Invalid("core.databases.default is required".into()))?;
		if !matches!(
			database.engine.as_str(),
			"postgresql" | "postgres" | "reinhardt.db.backends.postgresql"
		) {
			return Err(Error::Invalid("Aidash requires PostgreSQL".into()));
		}
		let node = &settings.node;
		let web_dir = settings.core.base_dir.join(&node.web_dir);
		Ok(Self {
			node_id: node.node_id.clone(),
			endpoint: node.endpoint.clone(),
			database_url: database.to_url(),
			nats_url: node.nats_url.clone(),
			api_token: node.api_token.clone(),
			web_dir: web_dir.to_string_lossy().into_owned(),
			lease_seconds: node.lease_seconds,
			default_host_packages: node.default_host_packages.clone(),
			oidc: settings
				.dashboard
				.oidc
				.as_ref()
				.map(OidcConfig::normalized)
				.transpose()?,
		})
	}
	pub fn identity(&self, clusters: Vec<String>) -> NodeIdentity {
		NodeIdentity {
			id: self.node_id.clone(),
			endpoint: self.endpoint.clone(),
			capabilities: vec![
				"agent.discovery".into(),
				"task.execute".into(),
				"workspace.share".into(),
			],
			clusters,
			protocol_version: PROTOCOL_VERSION.into(),
		}
	}
}

pub fn validate_node_id(value: &str) -> Result<()> {
	aidash_domain::configuration::validate_node_id(value).map_err(Into::into)
}

pub fn validate_endpoint(value: &str) -> Result<()> {
	aidash_domain::configuration::validate_endpoint(value).map_err(Into::into)
}

pub fn validate_secret_reference(value: &str) -> Result<()> {
	aidash_domain::configuration::validate_secret_reference(value).map_err(Into::into)
}
pub fn secret(name: &str) -> Result<String> {
	validate_secret_reference(name)?;
	env::var(name)
		.map_err(|_| Error::Invalid(format!("credential reference {name} is not configured")))
}

/// Peer bearer tokens must remain strong after environment-based rotation too.
pub fn peer_secret(name: &str) -> Result<String> {
	let value = secret(name)?;
	validate_peer_credential(&value)?;
	Ok(value)
}
pub fn validate_peer_credential(value: &str) -> Result<()> {
	aidash_domain::configuration::validate_peer_credential(value).map_err(Into::into)
}

pub(crate) use aidash_domain::configuration::same_secret;

pub use crate::apps::federation::peer::serializers::identity::NodeIdentity;

#[cfg(test)]
#[path = "../apps/execution/tests/config_runtime.rs"]
mod tests;
