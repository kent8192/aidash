use crate::{Error, Result};
use serde::Serialize;
use std::{env, net::SocketAddr};

pub(crate) const GOOGLE_OIDC_ISSUER: &str = "https://accounts.google.com";

#[derive(Clone)]
pub struct Config {
	pub node_id: String,
	pub endpoint: String,
	pub listen: SocketAddr,
	pub database_url: String,
	pub nats_url: String,
	pub api_token: String,
	pub web_dir: String,
	pub lease_seconds: i32,
	pub oidc: Option<OidcConfig>,
}

#[derive(Clone)]
pub struct OidcConfig {
	pub issuer: String,
	pub client_id: String,
	pub client_secret: String,
	pub public_origin: String,
	pub keycloak_admin_url: String,
	pub status_client_id: String,
	pub status_client_secret: String,
	pub session_absolute_seconds: i64,
	pub session_idle_seconds: i64,
}

impl OidcConfig {
	pub fn is_google(&self) -> bool {
		self.issuer == GOOGLE_OIDC_ISSUER
	}

	pub fn from_env() -> Result<Option<Self>> {
		Self::from_values(|key| env::var(key).ok())
	}

	fn from_values(value: impl Fn(&str) -> Option<String>) -> Result<Option<Self>> {
		let keys = [
			"AIDASH_OIDC_ISSUER",
			"AIDASH_OIDC_CLIENT_ID",
			"AIDASH_OIDC_CLIENT_SECRET",
			"AIDASH_OIDC_PUBLIC_ORIGIN",
			"AIDASH_OIDC_KEYCLOAK_ADMIN_URL",
			"AIDASH_OIDC_STATUS_CLIENT_ID",
			"AIDASH_OIDC_STATUS_CLIENT_SECRET",
		];
		let present = keys.iter().any(|key| value(key).is_some());
		if !present {
			return Ok(None);
		}
		let required = |key: &str| {
			value(key)
				.filter(|value| !value.trim().is_empty())
				.ok_or_else(|| {
					Error::Invalid(format!("{key} is required when dashboard OIDC is enabled"))
				})
		};
		let issuer = value(keys[0]).unwrap_or_else(|| GOOGLE_OIDC_ISSUER.into());
		let google = issuer == GOOGLE_OIDC_ISSUER;
		let client_id = required(keys[1])?;
		let client_secret = required(keys[2])?;
		let public_origin = required(keys[3])?;
		let keycloak_admin_url = if google {
			String::new()
		} else {
			required(keys[4])?
		};
		let status_client_id = if google {
			String::new()
		} else {
			required(keys[5])?
		};
		let status_client_secret = if google {
			String::new()
		} else {
			required(keys[6])?
		};
		for (label, value) in [
			("AIDASH_OIDC_ISSUER", &issuer),
			("AIDASH_OIDC_PUBLIC_ORIGIN", &public_origin),
			("AIDASH_OIDC_KEYCLOAK_ADMIN_URL", &keycloak_admin_url),
		] {
			if google && label == "AIDASH_OIDC_KEYCLOAK_ADMIN_URL" {
				continue;
			}
			let url = reqwest::Url::parse(value)
				.map_err(|_| Error::Invalid(format!("invalid {label}")))?;
			let secure = url.scheme() == "https";
			let local =
				url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
			if (!secure && !local)
				|| url.username() != ""
				|| url.password().is_some()
				|| url.query().is_some()
				|| url.fragment().is_some()
			{
				return Err(Error::Invalid(format!("invalid {label}")));
			}
		}
		let origin_url = reqwest::Url::parse(&public_origin)
			.map_err(|_| Error::Invalid("invalid AIDASH_OIDC_PUBLIC_ORIGIN".into()))?;
		if origin_url.path() != "/" {
			return Err(Error::Invalid(
				"AIDASH_OIDC_PUBLIC_ORIGIN must not contain a path".into(),
			));
		}
		let lifetime = |key: &str, default: i64| -> Result<i64> {
			let value = value(key).map_or(Ok(default), |value| {
				value
					.parse::<i64>()
					.map_err(|_| Error::Invalid(format!("invalid {key}")))
			})?;
			if !(60..=604_800).contains(&value) {
				return Err(Error::Invalid(format!("{key} must be 60..604800 seconds")));
			}
			Ok(value)
		};
		let session_absolute_seconds = lifetime("AIDASH_OIDC_SESSION_ABSOLUTE_SECONDS", 43_200)?;
		let session_idle_seconds = lifetime("AIDASH_OIDC_SESSION_IDLE_SECONDS", 1_800)?;
		if session_idle_seconds > session_absolute_seconds {
			return Err(Error::Invalid(
				"OIDC session idle time cannot exceed its absolute lifetime".into(),
			));
		}
		Ok(Some(Self {
			issuer,
			client_id,
			client_secret,
			public_origin: origin_url.origin().ascii_serialization(),
			keycloak_admin_url: keycloak_admin_url.trim_end_matches('/').to_string(),
			status_client_id,
			status_client_secret,
			session_absolute_seconds,
			session_idle_seconds,
		}))
	}
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct NodeIdentity {
	pub id: String,
	pub endpoint: String,
	pub capabilities: Vec<String>,
	pub clusters: Vec<String>,
	pub protocol_version: String,
}

pub const PROTOCOL_VERSION: &str = "0.1";

impl Config {
	pub fn from_env() -> Result<Self> {
		let required =
			|key: &str| env::var(key).map_err(|_| Error::Invalid(format!("{key} is required")));
		let node_id = required("AIDASH_NODE_ID")?;
		validate_node_id(&node_id)?;
		let endpoint = required("AIDASH_ENDPOINT")?;
		validate_endpoint(&endpoint)?;
		let api_token = required("AIDASH_API_TOKEN")?;
		if api_token.len() < 16 {
			return Err(Error::Invalid(
				"AIDASH_API_TOKEN must be at least 16 characters".into(),
			));
		}
		Ok(Self {
			node_id,
			endpoint,
			listen: env::var("AIDASH_LISTEN")
				.unwrap_or_else(|_| "127.0.0.1:8080".into())
				.parse()
				.map_err(|_| Error::Invalid("invalid AIDASH_LISTEN".into()))?,
			database_url: required("DATABASE_URL")?,
			nats_url: env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into()),
			api_token,
			web_dir: env::var("AIDASH_WEB_DIR").unwrap_or_else(|_| "web/dist".into()),
			lease_seconds: 30,
			oidc: OidcConfig::from_env()?,
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

pub fn validate_node_id(id: &str) -> Result<()> {
	let suffix = id.strip_prefix("aidash://").unwrap_or_default();
	if suffix.is_empty()
		|| suffix.len() > 100
		|| !suffix
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || c == '-')
	{
		return Err(Error::Invalid(
			"node id must be aidash:// followed by letters, numbers or hyphens".into(),
		));
	}
	Ok(())
}

pub fn validate_endpoint(endpoint: &str) -> Result<()> {
	let url =
		reqwest::Url::parse(endpoint).map_err(|_| Error::Invalid("invalid endpoint URL".into()))?;
	if !matches!(url.scheme(), "http" | "https")
		|| url.host_str().is_none()
		|| !url.username().is_empty()
		|| url.password().is_some()
		|| url.query().is_some()
		|| url.fragment().is_some()
	{
		return Err(Error::Invalid(
			"endpoint requires HTTP(S) without inline credentials, query or fragment".into(),
		));
	}
	Ok(())
}

pub fn validate_secret_reference(name: &str) -> Result<()> {
	if !name.starts_with("AIDASH_SECRET_")
		|| !name
			.chars()
			.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
	{
		return Err(Error::Invalid(
			"credential references must use AIDASH_SECRET_* environment variables".into(),
		));
	}
	Ok(())
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

#[cfg(test)]
mod tests {
	use super::*;
	#[rstest::rstest]
	fn google_oidc_needs_only_web_client_credentials_and_origin() {
		let values = [
			(
				"AIDASH_OIDC_CLIENT_ID",
				"web-client.apps.googleusercontent.com",
			),
			("AIDASH_OIDC_CLIENT_SECRET", "secret"),
			("AIDASH_OIDC_PUBLIC_ORIGIN", "https://example.com:443/"),
		];
		let read = |key: &str| {
			values
				.iter()
				.find(|(name, _)| *name == key)
				.map(|(_, v)| v.to_string())
		};
		let config = OidcConfig::from_values(read).unwrap().unwrap();
		assert_eq!(config.issuer, "https://accounts.google.com");
		assert_eq!(config.public_origin, "https://example.com");
		assert!(OidcConfig::from_values(|_| None).unwrap().is_none());
		for missing in [
			"AIDASH_OIDC_CLIENT_ID",
			"AIDASH_OIDC_CLIENT_SECRET",
			"AIDASH_OIDC_PUBLIC_ORIGIN",
		] {
			assert!(
				OidcConfig::from_values(|key| if key == missing { None } else { read(key) })
					.is_err()
			);
		}
		for (key, value) in [
			("AIDASH_OIDC_PUBLIC_ORIGIN", "https://example.com/path"),
			("AIDASH_OIDC_PUBLIC_ORIGIN", "http://example.com"),
			(
				"AIDASH_OIDC_ISSUER",
				"https://accounts.google.com.attacker.example",
			),
			("AIDASH_OIDC_SESSION_IDLE_SECONDS", "43201"),
		] {
			assert!(
				OidcConfig::from_values(|name| if name == key {
					Some(value.into())
				} else {
					read(name)
				})
				.is_err()
			);
		}
	}

	#[rstest::rstest]
	fn peer_credentials_reject_short_or_repeated_values() {
		for value in [
			"",
			"short",
			"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
			"abababababababababababababababab",
		] {
			assert!(validate_peer_credential(value).is_err());
		}
		assert!(
			validate_peer_credential(
				"839b16e2f81e28c65ded5407c407305769b28bb14c80a05439d701368994b66c"
			)
			.is_ok()
		);
	}
}
