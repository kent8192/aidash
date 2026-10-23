//! Another Deployment's `/.well-known/aidash`, fetched when a Node Registration
//! pins its key. A Tenant Administrator types the endpoint, so the fetch is as
//! constrained as a Keyless endpoint: see [`Endpoints`]. Every failure answers
//! one generic reason, so the route cannot probe hosts or ports.
use super::endpoint::Endpoints;
use aidash_application::{
	Error, Result, ports::federation::registrations::DeploymentDirectory,
	provider_access::KeylessAccess,
};
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;

/// What every failure of the fetch answers.
pub const UNREACHABLE: &str = "the endpoint could not be reached or did not answer as a Deployment";

pub struct HttpDirectory {
	endpoints: Endpoints,
}

impl HttpDirectory {
	/// `access` holds the origins the Deployment permits.
	pub fn new(access: KeylessAccess) -> Result<Self> {
		Ok(Self {
			endpoints: Endpoints::new(access)?,
		})
	}

	async fn fetch(&self, endpoint: &str) -> Result<Value> {
		let response = self
			.endpoints
			.client(endpoint)?
			.get(format!(
				"{}/.well-known/aidash",
				endpoint.trim_end_matches('/')
			))
			.timeout(Duration::from_secs(5))
			.send()
			.await
			.map_err(crate::keyless::send_error)?
			.error_for_status()
			.map_err(crate::http_error)?;
		crate::response::json(response, 1_048_576).await
	}
}

#[async_trait]
impl DeploymentDirectory for HttpDirectory {
	async fn identity(&self, endpoint: &str) -> Result<Value> {
		self.fetch(endpoint).await.map_err(|error| {
			tracing::warn!(%endpoint, %error, "Node Registration endpoint fetch failed");
			Error::Unprocessable(UNREACHABLE.into())
		})
	}
}

#[cfg(test)]
mod tests;
