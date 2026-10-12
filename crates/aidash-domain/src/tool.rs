//! External tool transport and replay contracts.
use crate::registry::EntityRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub mod concurrency;
pub use concurrency::Concurrency;
pub mod contract;
pub use contract::*;
pub mod providers;

/// The native Agent cutover still consumes the original tagged transport shape.
/// Select it only by its string tag; malformed descriptors never fall back to it.
pub fn legacy_config(value: &serde_json::Value) -> crate::Result<Option<ToolConfig>> {
	if value
		.get("transport")
		.is_some_and(serde_json::Value::is_string)
	{
		serde_json::from_value(value.clone())
			.map(Some)
			.map_err(|error| crate::Error::Invalid(error.to_string()))
	} else {
		Ok(None)
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "transport", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolConfig {
	Native {
		operation: String,
		#[serde(default)]
		allowed_hosts: Vec<String>,
	},
	Http {
		endpoint: String,
		credential_env: Option<String>,
		replay: String,
	},
	Mcp {
		endpoint: String,
		credential_env: Option<String>,
		tool_name: String,
		replay: String,
		idempotency_argument: Option<String>,
	},
	Agent {
		node_id: String,
		agent: EntityRef,
	},
}
