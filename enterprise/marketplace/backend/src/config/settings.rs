use crate::{Error, Result};
use std::env;
#[derive(Clone)]
pub struct Config {
	pub origin: String,
	pub issuer: String,
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
			origin: get("AIDASH_MARKETPLACE_ORIGIN")?,
			issuer: get("AIDASH_AUTH_ISSUER")?,
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
