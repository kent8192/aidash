//! External service adapters implementing application ports.
pub mod decision;
pub mod federation;
pub mod inference;
pub mod keyless;
pub mod kubernetes;
pub mod openrouter;
pub mod response;
pub mod semantic;

pub use aidash_application::{Error, Result};

pub fn http_error(error: reqwest::Error) -> Error {
	// External URLs can contain credentials; never retain them in errors.
	Error::External(error.without_url().to_string())
}

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

pub mod nats;

pub mod tools;

pub mod skill_import;

pub mod agent_plugin_import;

pub mod runner;

pub mod outbound;

pub mod sandbox;

pub mod activation;

pub mod oidc;

pub mod jwks;

pub mod chatgpt;
pub mod provider_authorization;

pub mod provider_credentials;

pub mod research;
