// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

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
	pub trusted_proxy_ips: Vec<ProxyNetwork>,
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
/// A trusted proxy network. A plain IP is a full-length prefix and matches only itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct ProxyNetwork {
	address: std::net::IpAddr,
	prefix: u32,
}
impl ProxyNetwork {
	fn width(address: std::net::IpAddr) -> u32 {
		if address.is_ipv4() { 32 } else { 128 }
	}
	fn bits(address: std::net::IpAddr) -> u128 {
		match address {
			std::net::IpAddr::V4(address) => u32::from(address).into(),
			std::net::IpAddr::V6(address) => address.into(),
		}
	}
	fn masked(&self, address: std::net::IpAddr) -> u128 {
		let host = Self::width(address) - self.prefix;
		Self::bits(address) & u128::MAX.checked_shl(host).unwrap_or(0)
	}
	pub fn contains(&self, address: std::net::IpAddr) -> bool {
		address.is_ipv4() == self.address.is_ipv4()
			&& self.masked(address) == Self::bits(self.address)
	}
}
impl std::str::FromStr for ProxyNetwork {
	type Err = String;
	fn from_str(raw: &str) -> Result<Self, Self::Err> {
		let invalid = || {
			format!(
				"invalid trusted proxy network {raw:?}; expected an IP or a CIDR network without host bits"
			)
		};
		let (address, prefix) = raw
			.split_once('/')
			.map_or((raw, None), |(a, p)| (a, Some(p)));
		let address: std::net::IpAddr = address.parse().map_err(|_| invalid())?;
		let width = Self::width(address);
		let prefix = match prefix {
			None => width,
			Some(prefix) if !prefix.is_empty() && prefix.bytes().all(|b| b.is_ascii_digit()) => {
				prefix
					.parse()
					.ok()
					.filter(|p| *p <= width)
					.ok_or_else(invalid)?
			}
			Some(_) => return Err(invalid()),
		};
		let network = Self { address, prefix };
		// Host bits set usually mean a mistyped network; refuse rather than widen it.
		if network.masked(address) != Self::bits(address) {
			return Err(invalid());
		}
		Ok(network)
	}
}
impl TryFrom<String> for ProxyNetwork {
	type Error = String;
	fn try_from(raw: String) -> Result<Self, String> {
		raw.parse()
	}
}
/// The peer address when it lies in a trusted network. Reinhardt then honours the forwarded
/// headers for that peer alone; any other peer gets an empty trusted set.
pub fn trusted_peer(
	networks: &[ProxyNetwork],
	peer: Option<std::net::SocketAddr>,
) -> Option<std::net::IpAddr> {
	peer.map(|peer| peer.ip())
		.filter(|ip| networks.iter().any(|network| network.contains(*ip)))
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
		if self.gcip_auth_domain.len() > 253
			|| self.gcip_auth_domain.split('.').any(|label| {
				label.is_empty()
					|| label.len() > 63
					|| label.starts_with('-')
					|| label.ends_with('-')
					|| !label
						.bytes()
						.all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
			}) {
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
	fn startup_accepts_auth_hosts_and_rejects_csp_source_injection() {
		let mut value = config();
		for host in ["project.firebaseapp.com", "login.example.com", "localhost"] {
			value.gcip_auth_domain = host.into();
			assert!(value.validate().is_ok(), "valid auth host: {host}");
		}
		for host in [
			"https://login.example.com",
			"login.example.com/path",
			"login.example.com:443",
			"login.example.com; script-src *",
			"login.example.com\r\nX-Injected: yes",
			"login..example.com",
			"-login.example.com",
		] {
			value.gcip_auth_domain = host.into();
			assert!(value.validate().is_err(), "invalid auth host: {host:?}");
		}
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
	#[test]
	fn proxy_network_contains_only_its_own_family_and_range() {
		let network = |raw: &str| raw.parse::<ProxyNetwork>().unwrap();
		let ip = |raw: &str| raw.parse::<std::net::IpAddr>().unwrap();
		for (cidr, address, expected) in [
			("10.44.0.0/14", "10.44.0.1", true),
			("10.44.0.0/14", "10.47.255.255", true),
			("10.44.0.0/14", "10.48.0.0", false),
			("10.44.0.0/14", "10.43.255.255", false),
			("127.0.0.0/8", "127.0.0.1", true),
			("10.0.0.0/8", "127.0.0.1", false),
			("0.0.0.0/0", "203.0.113.9", true),
			("0.0.0.0/0", "::1", false),
			("127.0.0.1", "127.0.0.1", true),
			("127.0.0.1", "127.0.0.2", false),
			("fd00::/8", "fd12::1", true),
			("fd00::/8", "fe80::1", false),
			("::1", "::1", true),
			("::/0", "::1", true),
			("::/0", "127.0.0.1", false),
			("::1", "127.0.0.1", false),
		] {
			assert_eq!(
				network(cidr).contains(ip(address)),
				expected,
				"{cidr} contains {address}"
			);
		}
	}
	#[test]
	fn proxy_network_rejects_malformed_entries() {
		for raw in [
			"10.0.0.1/8",
			"10.44.0.0/33",
			"fd00::/129",
			"fd00::1/8",
			"10.0.0.0/",
			"10.0.0.0/+8",
			"10.0.0.0/-1",
			"10.0.0.0/8/8",
			"10.0.0.0/ 8",
			"/8",
			"",
			"edge.example.com",
			"10.0.0.0/eight",
		] {
			assert!(raw.parse::<ProxyNetwork>().is_err(), "{raw:?}");
		}
	}
	#[test]
	fn trusted_peer_requires_a_matching_network() {
		let networks: Vec<ProxyNetwork> =
			vec!["10.0.0.0/8".parse().unwrap(), "::1".parse().unwrap()];
		let peer = |raw: &str| Some(raw.parse::<std::net::SocketAddr>().unwrap());
		assert_eq!(
			trusted_peer(&networks, peer("10.1.2.3:443")),
			Some("10.1.2.3".parse().unwrap())
		);
		assert_eq!(
			trusted_peer(&networks, peer("[::1]:443")),
			Some("::1".parse().unwrap())
		);
		assert_eq!(trusted_peer(&networks, peer("192.0.2.1:443")), None);
		assert_eq!(trusted_peer(&networks, None), None);
		assert_eq!(trusted_peer(&[], peer("10.1.2.3:443")), None);
	}
}
