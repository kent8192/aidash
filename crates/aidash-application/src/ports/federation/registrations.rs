//! Asking another Deployment who it is, for a Node Registration to pin.
use crate::Result;
use async_trait::async_trait;
use serde_json::Value;

#[async_trait]
pub trait DeploymentDirectory: Send + Sync {
	/// The `/.well-known/aidash` document served at `endpoint`.
	async fn identity(&self, endpoint: &str) -> Result<Value>;
}
