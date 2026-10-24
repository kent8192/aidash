//! Explicit first-party service configuration; secrets are loaded from mounted files.
use serde::Deserialize;
use std::{
	collections::{BTreeMap, HashSet},
	net::SocketAddr,
	path::PathBuf,
};
use url::Url;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grpc {
	pub listen: SocketAddr,
	pub certificate_file: PathBuf,
	pub private_key_file: PathBuf,
	pub client_ca_file: PathBuf,
	pub product_peers: BTreeMap<String, String>,
	pub administrator_peers: Vec<String>,
	#[serde(default)]
	pub source_restore_clients: Vec<String>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Client {
	pub id: String,
	pub redirect_uri: String,
	pub secret_file: PathBuf,
	pub introspection_secret_file: PathBuf,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
	pub issuer: String,
	pub listen: SocketAddr,
	pub database_url_file: PathBuf,
	pub schema: String,
	pub gcip_project: String,
	pub gcip_tenants: Vec<String>,
	#[serde(default)]
	pub gcip_providers: BTreeMap<String, Vec<String>>,
	#[serde(default)]
	pub gcip_password_sign_up: bool,
	#[serde(default)]
	pub gcip_mfa_mandatory: Vec<String>,
	#[serde(default)]
	pub trusted_proxy_ips: Vec<std::net::IpAddr>,
	#[serde(default)]
	pub assets_dir: Option<PathBuf>,
	pub gcip_browser_api_key: String,
	pub gcip_auth_domain: String,
	pub signing_key_file: PathBuf,
	pub token_encryption_key_file: PathBuf,
	pub grpc: Grpc,
	pub signing_key_id: String,
	pub clients: Vec<Client>,
	#[serde(default)]
	pub loopback_development: bool,
}
impl Config {
	pub fn validate(&self) -> Result<(), String> {
		let invalid = || "invalid authentication service configuration".to_string();
		let issuer = Url::parse(&self.issuer).map_err(|_| invalid())?;
		let secure_url = |url: &Url| {
			url.scheme() == "https"
				|| self.loopback_development
					&& url.scheme() == "http"
					&& matches!(
						url.host_str(),
						Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
					)
		};
		if !secure_url(&issuer)
			|| issuer.path() != "/"
			|| issuer.query().is_some()
			|| issuer.fragment().is_some()
			|| !issuer.username().is_empty()
			|| issuer.password().is_some()
			|| issuer.host_str().is_none()
			|| self.issuer.ends_with('/')
		{
			return Err(invalid());
		}
		if self.schema != "aidash_auth"
			|| self.gcip_project.is_empty()
			|| self.gcip_tenants.is_empty()
			|| self
				.gcip_tenants
				.iter()
				.any(|v| v.is_empty() || v.contains('/'))
			|| self.gcip_tenants.iter().collect::<HashSet<_>>().len() != self.gcip_tenants.len()
			|| self.signing_key_id.is_empty()
			|| self.clients.is_empty()
			|| self.gcip_browser_api_key.is_empty()
			|| self.gcip_auth_domain.is_empty()
		{
			return Err(invalid());
		}
		if self.gcip_providers.iter().any(|(tenant, providers)| {
			!self.gcip_tenants.contains(tenant)
				|| providers.is_empty()
				|| providers.iter().any(|provider| provider.is_empty())
				|| providers.iter().collect::<HashSet<_>>().len() != providers.len()
		}) || self
			.gcip_mfa_mandatory
			.iter()
			.any(|tenant| !self.gcip_tenants.contains(tenant))
		{
			return Err(invalid());
		}
		let mut ids = HashSet::new();
		for client in &self.clients {
			let redirect = Url::parse(&client.redirect_uri).map_err(|_| invalid())?;
			if !matches!(client.id.as_str(), "cloud" | "marketplace")
				|| !ids.insert(&client.id)
				|| !secure_url(&redirect)
				|| redirect.fragment().is_some()
				|| redirect.query().is_some()
				|| !redirect.username().is_empty()
				|| redirect.password().is_some()
				|| redirect.host_str().is_none()
				|| redirect.origin() == issuer.origin()
			{
				return Err(invalid());
			}
		}
		if self
			.grpc
			.source_restore_clients
			.iter()
			.any(|client| !self.grpc.product_peers.contains_key(client))
		{
			return Err(invalid());
		}
		if self.grpc.product_peers.is_empty()
			|| self
				.grpc
				.product_peers
				.values()
				.collect::<HashSet<_>>()
				.len() != self.grpc.product_peers.len()
			|| self.grpc.product_peers.iter().any(|(client, peer)| {
				!matches!(client.as_str(), "cloud" | "marketplace")
					|| !peer.starts_with("spiffe://")
					|| self.grpc.administrator_peers.contains(peer)
			}) || self
			.grpc
			.administrator_peers
			.iter()
			.any(|peer| !peer.starts_with("spiffe://"))
		{
			return Err(invalid());
		}
		Ok(())
	}
	pub fn read(path: &std::path::Path) -> Result<Self, String> {
		let text = std::fs::read_to_string(path)
			.map_err(|_| "authentication configuration unavailable")?;
		let value: Self =
			toml::from_str(&text).map_err(|_| "invalid authentication configuration")?;
		value.validate()?;
		Ok(value)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	fn config() -> Config {
		Config {
			issuer: "https://auth.example".into(),
			listen: "127.0.0.1:8081".parse().unwrap(),
			database_url_file: "db.secret".into(),
			schema: "aidash_auth".into(),
			gcip_project: "project".into(),
			gcip_tenants: vec!["shared".into()],
			gcip_providers: BTreeMap::new(),
			gcip_password_sign_up: false,
			gcip_mfa_mandatory: vec![],
			trusted_proxy_ips: vec![],
			assets_dir: None,
			gcip_browser_api_key: "public-key".into(),
			gcip_auth_domain: "project.firebaseapp.com".into(),
			signing_key_file: "rsa.secret".into(),
			token_encryption_key_file: "encryption.secret".into(),
			grpc: Grpc {
				listen: "127.0.0.1:9091".parse().unwrap(),
				certificate_file: "cert.pem".into(),
				private_key_file: "key.pem".into(),
				client_ca_file: "ca.pem".into(),
				product_peers: [("cloud".into(), "spiffe://aidash/cloud".into())].into(),
				administrator_peers: vec!["spiffe://aidash/auth-admin".into()],
				source_restore_clients: vec!["cloud".into()],
			},
			signing_key_id: "key-1".into(),
			clients: vec![Client {
				id: "cloud".into(),
				redirect_uri: "https://cloud.example/callback".into(),
				secret_file: "cloud.secret".into(),
				introspection_secret_file: "resource.secret".into(),
			}],
			loopback_development: false,
		}
	}
	#[test]
	fn startup_rejects_unregistered_product_and_untrusted_callback() {
		let mut value = config();
		assert!(value.validate().is_ok());
		value.clients[0].id = "third-party".into();
		assert!(
			value.validate().is_err(),
			"initial issuer must accept first-party clients only"
		);
		value.clients[0].id = "cloud".into();
		value.clients[0].redirect_uri = "https://cloud.example/callback#token".into();
		assert!(
			value.validate().is_err(),
			"a registered callback cannot contain a fragment"
		);
		value.clients[0].redirect_uri = "http://cloud.example/callback".into();
		assert!(
			value.validate().is_err(),
			"production callback requires HTTPS"
		);
	}
	#[test]
	fn startup_rejects_ambiguous_peer_and_unknown_upstream_pool() {
		let mut value = config();
		value.grpc.product_peers.insert(
			"marketplace".into(),
			value.grpc.product_peers["cloud"].clone(),
		);
		assert!(
			value.validate().is_err(),
			"two products cannot share a transport identity"
		);
		value.grpc.product_peers.remove("marketplace");
		value
			.gcip_providers
			.insert("misspelled-pool".into(), vec!["password".into()]);
		assert!(
			value.validate().is_err(),
			"a misspelled pool must not silently use the default sign-in methods"
		);
	}
}
