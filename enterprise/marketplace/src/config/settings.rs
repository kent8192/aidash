use crate::{Error, Result};
use std::env;
#[derive(Clone)]
pub struct Config {
	pub assets_dir: Option<std::path::PathBuf>,
	pub origin: String,
	pub issuer: String,
	/// IPs or CIDR networks of the trusted HTTPS reverse proxy; only they may assert forwarded HTTPS.
	pub trusted_proxy_ips: Vec<ProxyNetwork>,
	pub client_id: String,
	pub client_secret: String,
	pub billing_secret: String,
	pub enrollment_secret: String,
	pub recall_secret: String,
	pub resource_client_id: String,
	pub resource_secret: String,
}
impl Config {
	pub fn from_env() -> Result<Self> {
		let get = |key: &str| {
			env::var(key).map_err(|_| Error::Invalid("missing Marketplace configuration"))
		};
		let mut config = Self {
			assets_dir: env::var_os("AIDASH_MARKETPLACE_ASSETS_DIR").map(Into::into),
			origin: get("AIDASH_MARKETPLACE_ORIGIN")?,
			issuer: get("AIDASH_AUTH_ISSUER")?,
			trusted_proxy_ips: trusted_proxy_ips()?,
			client_id: env::var("AIDASH_MARKETPLACE_CLIENT_ID")
				.unwrap_or_else(|_| "marketplace".into()),
			client_secret: read_secret("AIDASH_MARKETPLACE_CLIENT_SECRET_FILE")?,
			billing_secret: read_secret("AIDASH_MARKETPLACE_BILLING_SECRET_FILE")?,
			enrollment_secret: read_secret("AIDASH_MARKETPLACE_ENROLLMENT_SECRET_FILE")?,
			recall_secret: read_secret("AIDASH_MARKETPLACE_RECALL_SECRET_FILE")?,
			resource_client_id: env::var("AIDASH_MARKETPLACE_RESOURCE_CLIENT_ID")
				.unwrap_or_else(|_| "marketplace-resource".into()),
			resource_secret: read_secret("AIDASH_MARKETPLACE_RESOURCE_SECRET_FILE")?,
		};
		for value in [&config.origin, &config.issuer] {
			let url = url::Url::parse(value).map_err(|_| Error::Invalid("invalid service URL"))?;
			if (url.scheme() != "https"
				&& !(url.scheme() == "http"
					&& matches!(url.host_str(), Some("localhost" | "127.0.0.1"))))
				|| url.query().is_some()
				|| url.fragment().is_some()
				|| !url.username().is_empty()
				|| url.password().is_some()
			{
				return Err(Error::Invalid(
					"service URLs require HTTPS except loopback development",
				));
			}
		}
		let origin = url::Url::parse(&config.origin)
			.map_err(|_| Error::Invalid("invalid Marketplace origin"))?;
		if origin.path() != "/" {
			return Err(Error::Invalid("Marketplace origin must not contain a path"));
		}
		config.origin = origin.origin().ascii_serialization();
		Ok(config)
	}
	pub fn callback(&self) -> String {
		format!("{}/auth/callback", self.origin.trim_end_matches('/'))
	}
	pub fn secure(&self) -> bool {
		self.origin.starts_with("https://")
	}
}
fn trusted_proxy_ips() -> Result<Vec<ProxyNetwork>> {
	let Ok(value) = env::var("AIDASH_MARKETPLACE_TRUSTED_PROXY_IPS") else {
		return Ok(Vec::new());
	};
	value
		.split(',')
		.map(str::trim)
		.filter(|entry| !entry.is_empty())
		.map(str::parse)
		.collect()
}
/// A trusted proxy network. A plain IP is a full-length prefix and matches only itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
	type Err = Error;
	fn from_str(raw: &str) -> std::result::Result<Self, Self::Err> {
		let invalid = || {
			Error::Invalid(
				"invalid AIDASH_MARKETPLACE_TRUSTED_PROXY_IPS entry; expected an IP or a CIDR network without host bits",
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
/// The peer address when it lies in a trusted network. Reinhardt then honours the forwarded
/// headers for that peer alone; any other peer gets an empty trusted set.
pub fn trusted_peer(
	networks: &[ProxyNetwork],
	peer: Option<std::net::SocketAddr>,
) -> Option<std::net::IpAddr> {
	peer.map(|peer| peer.ip())
		.filter(|ip| networks.iter().any(|network| network.contains(*ip)))
}
fn read_secret(key: &str) -> Result<String> {
	let path = env::var(key).map_err(|_| Error::Invalid("missing Marketplace secret file"))?;
	let secret = std::fs::read_to_string(path)
		.map_err(|_| Error::Invalid("cannot read Marketplace secret file"))?;
	let secret = secret.trim().to_owned();
	if secret.len() < 32 {
		return Err(Error::Invalid(
			"Marketplace service secrets require at least 32 characters",
		));
	}
	Ok(secret)
}
#[cfg(test)]
mod tests {
	use super::*;
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
