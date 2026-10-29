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

/// Whether a tool accepts a Tenant Secret shared by everyone, or only a
/// personal secret owned by the Run's chain-root person (ADR 0100).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SecretOwner {
	/// A personal secret wins; a shared Tenant Secret may be used otherwise.
	#[default]
	Any,
	/// Only the person's own secret may be used.
	Personal,
}

impl SecretOwner {
	pub fn is_any(&self) -> bool {
		matches!(self, Self::Any)
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
	/// to the hosts recorded with that Tenant Secret (ADR 0051), or with an MCP
	/// OAuth Connection (ADR 0100).
	Http {
		endpoint: String,
		replay: String,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		secret: Option<String>,
		#[serde(default, skip_serializing_if = "SecretOwner::is_any")]
		secret_owner: SecretOwner,
	},
	Mcp {
		endpoint: String,
		tool_name: String,
		replay: String,
		idempotency_argument: Option<String>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		secret: Option<String>,
		/// The OAuth scopes the tool declares; a person's Connection may request
		/// only these when they are present.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		scopes: Option<Vec<String>>,
		#[serde(default, skip_serializing_if = "SecretOwner::is_any")]
		secret_owner: SecretOwner,
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

#[cfg(test)]
mod tests;
