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

/// One remote server of an imported Agent Plugin copy to list for the person attaching or editing
/// it, so they can declare Read-only tools (ADR 0073, amended 2026-11-16).
#[derive(Debug, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginToolsRequest {
	/// The [`AgentPluginRef::digest`] of the Tenant's copy.
	pub digest: String,
	/// A remote server of that copy.
	pub server: aidash_domain::agent_plugin::ServerName,
	/// The pasted secret the Agent definition names for the server, used when the person has no
	/// OAuth Connection to it. Only the person's own secret of that name is used; a shared Tenant
	/// Secret never is, whatever `owner` says.
	#[serde(default)]
	pub secret: Option<aidash_domain::agent_plugin::ServerSecret>,
}

pub use aidash_application::plugin_tools::{AttachListing, AttachTool};
