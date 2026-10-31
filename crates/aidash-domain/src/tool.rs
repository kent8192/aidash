//! External tool transport and replay contracts.
use crate::registry::EntityRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub mod concurrency;
pub use concurrency::Concurrency;
pub mod contract;
pub use contract::*;
pub mod providers;
pub mod refusal;
pub use refusal::{AuthorityRefusal, RefusalCheck};

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
	/// Tool transports never carry a Deployment secret. A Tenant's tool
	/// authenticates with a Tenant Secret it names in `secret`, which goes only
	/// to the hosts recorded with that Tenant Secret (ADR 0051).
	Http {
		endpoint: String,
		replay: String,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		secret: Option<String>,
	},
	Mcp {
		endpoint: String,
		tool_name: String,
		replay: String,
		idempotency_argument: Option<String>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		secret: Option<String>,
	},
	Agent {
		node_id: String,
		agent: EntityRef,
	},
}

impl ToolConfig {
	/// The Tenant's definition version declares its one `invoke` operation a
	/// Read-only operation (ADR 0078). Any other `replay` value, and anything
	/// an MCP server says about itself, leaves the operation unproven.
	pub fn declares_read_only(&self) -> bool {
		matches!(
			self,
			Self::Http { replay, .. } | Self::Mcp { replay, .. } if replay == "read_only"
		)
	}
}
