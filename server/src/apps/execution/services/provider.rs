//! Compatibility exports and bootstrap access for inference callers.
use crate::{Result, registry::ModelConfig};
pub use aidash_application::ports::ModelProvider;
pub use aidash_domain::provider::{ContentPart, ModelRequest, ModelResponse, ToolCall, ToolSpec};
use std::sync::Arc;

/// A Legacy-only provider: salted requests are rejected without being sent.
pub fn provider(client: reqwest::Client, config: ModelConfig) -> Result<Arc<dyn ModelProvider>> {
	crate::bootstrap::model_provider(client, config, None)
}

pub fn parse_openai(value: serde_json::Value) -> Result<ModelResponse> {
	aidash_integrations::inference::parse_openai(value).map_err(Into::into)
}
