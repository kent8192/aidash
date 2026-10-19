//! HTTP contract of the Agent Plugin import.
use aidash_domain::agent_plugin::AgentPluginRef;
use schemars::JsonSchema;
use serde::Serialize;

pub use aidash_application::ports::registry::PluginImportRequest;

#[derive(Debug, Serialize, JsonSchema)]
pub struct AgentPluginImportResult {
	/// The plugin roots to choose from when the repository holds several, or
	/// the imported root.
	pub plugins: Vec<String>,
	/// The stored copy, absent while the caller still has to choose a root.
	pub selected: Option<ImportedAgentPlugin>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ImportedAgentPlugin {
	/// What the Agent definition records in `agent_plugins`.
	pub reference: AgentPluginRef,
	/// Paths the Agent Plugins specification forbids access to, left out of the copy.
	pub denied: Vec<String>,
}
