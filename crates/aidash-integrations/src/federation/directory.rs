//! Another Deployment's `/.well-known/aidash`, fetched when a Node Registration
//! pins its key.
use aidash_application::{Result, ports::federation::registrations::DeploymentDirectory};
use aidash_domain::configuration::validate_peer_endpoint;
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;

pub struct HttpDirectory {
	pub client: reqwest::Client,
}

#[async_trait]
impl DeploymentDirectory for HttpDirectory {
	async fn identity(&self, endpoint: &str) -> Result<Value> {
		validate_peer_endpoint(endpoint)?;
		let response = self
			.client
			.get(format!(
				"{}/.well-known/aidash",
				endpoint.trim_end_matches('/')
			))
			.timeout(Duration::from_secs(5))
			.send()
			.await
			.map_err(crate::http_error)?
			.error_for_status()
			.map_err(crate::http_error)?;
		crate::response::json(response, 1_048_576).await
	}
}
