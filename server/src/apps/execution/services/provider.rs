//! Compatibility exports and bootstrap access for inference callers.
use crate::{Result, registry::ModelConfig};
pub use aidash_application::ports::ModelProvider;
pub use aidash_domain::provider::{ContentPart, ModelRequest, ModelResponse, ToolCall, ToolSpec};
use std::sync::Arc;

/// A Legacy-only provider: salted requests are rejected without being sent.
/// A Keyless endpoint reaches a private address only through an origin that
/// `keyless` permits (ADR 0036).
pub fn provider(
	client: reqwest::Client,
	config: ModelConfig,
	keyless: aidash_application::provider_access::KeylessAccess,
) -> Result<Arc<dyn ModelProvider>> {
	crate::bootstrap::model_provider(client, config, keyless)
}

pub fn parse_openai(body: &[u8]) -> Result<ModelResponse> {
	aidash_integrations::inference::parse_openai(body).map_err(Into::into)
}
