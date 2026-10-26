//! The server-side Tenant Secret call adapter (ADR 0095). It mints a Tenant
//! Secret Capability Token and returns the broker base; it never reads a value.
use super::secret_issuer::{SecretIssuerConfiguration, SecretRequest, SecretTokenIssuer};
use super::{KmsTokenSigner, MetadataTokenSource};
use aidash_application::tenant_secrets::{SecretCallGrant, SecretCalls, SecretRoute};
use aidash_application::{Error, Result};
use aidash_capability::TokenSigner;
use async_trait::async_trait;
use std::sync::Arc;

/// Where the broker receives a forwarded call.
const FORWARD_PATH: &str = "/v1/forward";

pub struct SecretCallsAdapter {
	base: reqwest::Url,
	issuer: SecretTokenIssuer,
}
impl SecretCallsAdapter {
	/// The production adapter: a KMS signer for `configuration.kid`. On the
	/// server role that key is the server's own; on the worker role it is the
	/// worker's existing broker key.
	pub fn new(endpoint: &str, configuration: SecretIssuerConfiguration) -> Result<Self> {
		let signer = Arc::new(
			KmsTokenSigner::new(
				configuration.kid.clone(),
				Arc::new(MetadataTokenSource::new().map_err(|_| {
					Error::Invalid("Tenant Secret broker metadata configuration unavailable".into())
				})?),
			)
			.map_err(|_| Error::Invalid("invalid Tenant Secret signing key version".into()))?,
		);
		Self::with_signer(endpoint, configuration, signer)
	}
	/// A test seam, and the one constructor the production path uses.
	pub fn with_signer(
		endpoint: &str,
		configuration: SecretIssuerConfiguration,
		signer: Arc<dyn TokenSigner>,
	) -> Result<Self> {
		let base = forward_base(endpoint)?;
		Ok(Self {
			base,
			issuer: SecretTokenIssuer::new(configuration, signer)?,
		})
	}
}

#[async_trait]
impl SecretCalls for SecretCallsAdapter {
	async fn route(&self, grant: SecretCallGrant) -> Result<SecretRoute> {
		let request = SecretRequest {
			tenant: grant.tenant,
			secret: grant.secret,
			version: grant.version,
			destination: grant.destination,
			methods: grant.methods,
			attach: grant.attach,
			sub: grant.subject,
			max_response_bytes: grant.max_response_bytes,
		};
		let token = self.issuer.mint(&request).await?;
		Ok(SecretRoute {
			base: self.base.clone(),
			token,
		})
	}
}

/// `{endpoint}/v1/forward`. The endpoint is the bare service URI: HTTPS, or
/// HTTP only on a literal loopback address for local runs.
fn forward_base(endpoint: &str) -> Result<reqwest::Url> {
	let endpoint = endpoint.trim_end_matches('/');
	let url = reqwest::Url::parse(endpoint)
		.map_err(|_| Error::Invalid("invalid Tenant Secret broker endpoint URL".into()))?;
	let loopback = url
		.host_str()
		.map(|host| host.trim_start_matches('[').trim_end_matches(']'))
		.and_then(|host| host.parse::<std::net::IpAddr>().ok())
		.is_some_and(|ip| ip.is_loopback());
	if url.scheme() != "https" && !(url.scheme() == "http" && loopback)
		|| url.cannot_be_a_base()
		|| !url.username().is_empty()
		|| url.password().is_some()
		|| url.query().is_some()
		|| url.fragment().is_some()
		|| !matches!(url.path(), "" | "/")
	{
		return Err(Error::Invalid(
			"invalid Tenant Secret broker endpoint URL".into(),
		));
	}
	reqwest::Url::parse(&format!("{endpoint}{FORWARD_PATH}"))
		.map_err(|_| Error::Invalid("invalid Tenant Secret broker endpoint URL".into()))
}

#[cfg(test)]
mod tests;
