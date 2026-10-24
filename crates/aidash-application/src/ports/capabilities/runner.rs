//! Isolated execution uses a transport that contains no host command capability.
use crate::Result;
use async_trait::async_trait;
use serde_json::Value;
#[async_trait]
pub trait RunnerTransport: Send + Sync {
	async fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value>;
}
