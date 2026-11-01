//! Personal-authentication Federation messages carry call identity and arguments, never credentials.
use crate::{
	Error, Result,
	agent_plugin::{PluginName, ServerName},
	oauth_connections::canonical_resource,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CALL: &str = "mcp.connection_call";
pub const LIST: &str = "mcp.connection_list";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolKey {
	Registry {
		registry_deployment: String,
		id: String,
		version: String,
	},
	Plugin {
		plugin: PluginName,
		digest: String,
		server: ServerName,
		schema_digest: Option<String>,
	},
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
	Connection,
	PersonalSecret,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionCall {
	pub tool_key: ToolKey,
	pub resource: String,
	pub tool_name: String,
	pub arguments: Value,
	pub idempotency_key: String,
	pub kind: Kind,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub secret_name: Option<String>,
}
impl ConnectionCall {
	pub fn validate(&self, list: bool) -> Result<()> {
		if self.kind == Kind::Connection && canonical_resource(&self.resource)? != self.resource {
			return Err(Error::Invalid("noncanonical connection resource".into()));
		}
		if !list && (self.idempotency_key.is_empty() || self.idempotency_key.len() > 512) {
			return Err(Error::Invalid("invalid idempotency_key".into()));
		}
		if list
			&& (!matches!(self.tool_key, ToolKey::Plugin { .. })
				|| !self.tool_name.is_empty()
				|| self.arguments != serde_json::json!({}))
		{
			return Err(Error::Invalid("invalid connection listing".into()));
		}
		if self.kind == Kind::PersonalSecret && self.secret_name.is_none() {
			return Err(Error::Invalid("missing personal secret name".into()));
		}
		Ok(())
	}
}
/// Control outcomes are distinct from arbitrary tool JSON, which may itself
/// contain an `outcome` field. Only this outer envelope controls fallback.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
	Result {
		result: Value,
	},
	Tools {
		tools: Vec<crate::agent_plugin::ListedTool>,
	},
	Connection {
		cause: crate::agent_plugin::ConnectionCause,
	},
	NotPersonal,
}
/// Only typed control outcomes promise that no operation executed.
pub fn retryable(value: &Value) -> bool {
	matches!(
		serde_json::from_value::<Outcome>(value.clone()),
		Ok(Outcome::NotPersonal | Outcome::Connection { .. })
	)
}
