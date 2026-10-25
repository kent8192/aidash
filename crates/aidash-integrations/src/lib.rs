//! External service adapters implementing application ports.
#[cfg(feature = "cloud-adapters")]
pub mod decision;
#[cfg(feature = "cloud-adapters")]
pub mod federation;
#[cfg(feature = "cloud-adapters")]
pub mod inference;
#[cfg(feature = "cloud-adapters")]
pub mod keyless;
#[cfg(feature = "cloud-adapters")]
pub mod kubernetes;
#[cfg(feature = "cloud-adapters")]
pub mod openrouter;
#[cfg(feature = "cloud-adapters")]
pub mod response;
#[cfg(feature = "cloud-adapters")]
pub mod semantic;

pub use aidash_application::{Error, Result};

pub fn http_error(error: reqwest::Error) -> Error {
	// External URLs can contain credentials; never retain them in errors.
	Error::External(error.without_url().to_string())
}

#[cfg(feature = "cloud-adapters")]
pub(crate) fn validate_endpoint(endpoint: &str) -> Result<()> {
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

#[cfg(feature = "cloud-adapters")]
pub mod nats;

#[cfg(feature = "cloud-adapters")]
pub mod tools;

#[cfg(feature = "cloud-adapters")]
pub mod plugin_sessions;

#[cfg(feature = "cloud-adapters")]
pub mod skill_import;

#[cfg(feature = "cloud-adapters")]
pub mod agent_plugin_import;

#[cfg(feature = "cloud-adapters")]
pub mod runner;

#[cfg(feature = "cloud-adapters")]
pub mod outbound;

#[cfg(feature = "cloud-adapters")]
pub mod sandbox;

#[cfg(feature = "cloud-adapters")]
pub mod activation;

#[cfg(feature = "authentication")]
pub mod authentication;

pub mod oidc;

pub mod jwks;

#[cfg(feature = "cloud-adapters")]
pub mod chatgpt;
#[cfg(feature = "cloud-adapters")]
pub mod provider_authorization;

#[cfg(feature = "cloud-adapters")]
pub mod provider_credentials;

#[cfg(feature = "cloud-adapters")]
pub mod research;

#[cfg(feature = "cloud-adapters")]
pub mod egress;

#[cfg(feature = "cloud-adapters")]
pub mod marketplace;
