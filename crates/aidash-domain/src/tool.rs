//! External tool transport and replay contracts.
use crate::registry::EntityRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
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
