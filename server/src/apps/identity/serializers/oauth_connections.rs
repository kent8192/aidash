//! OAuth Connection API contracts. Responses contain metadata only.
use serde::{Deserialize, Serialize};
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ConnectionAuthorizationStart")]
pub struct Start {
	pub resource: String,
	#[serde(default)]
	pub scopes: Vec<String>,
	pub tool: Option<aidash_application::oauth_connections::ToolRef>,
	pub return_to: Option<String>,
}
#[derive(Serialize, schemars::JsonSchema)]
#[schemars(rename = "ConnectionAuthorizationStarted")]
pub struct Started {
	pub authorization_url: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
pub struct CallbackQuery {
	pub state: Option<String>,
	pub code: Option<String>,
	pub error: Option<String>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ConnectionProbeQuery")]
pub struct ProbeQuery {
	pub resource: String,
}

/// Tenant policy metadata readable with the probe's existing permission.
#[derive(Serialize, schemars::JsonSchema)]
#[schemars(rename = "ConnectionProbe")]
pub struct Probe {
	pub oauth: bool,
	pub scopes_supported: Option<Vec<String>>,
	pub issuer: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub scope_ceiling: Option<Vec<String>>,
}
