use serde::{Deserialize, Serialize};
// Serializable errors contracts.
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct CapabilityError {
	pub code: String,
	pub message: String,
	pub retryable: bool,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub details: Option<Value>,
}
