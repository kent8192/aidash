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
	if value.len() < 32
		|| !value.bytes().all(|b| b.is_ascii_graphic())
		|| value
			.bytes()
			.collect::<std::collections::HashSet<_>>()
			.len() < 8
	{
		return Err(Error::Invalid("peer credentials require at least 32 ASCII characters and 8 distinct characters; use a randomly generated token".into()));
	}
	Ok(())
}

pub(crate) fn same_secret(a: &str, b: &str) -> bool {
	use sha2::{Digest, Sha256};
	let a = Sha256::digest(a.as_bytes());
	let b = Sha256::digest(b.as_bytes());
	a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub use crate::apps::federation::peer::serializers::identity::NodeIdentity;

#[cfg(test)]
#[path = "../apps/execution/tests/config_runtime.rs"]
mod tests;
