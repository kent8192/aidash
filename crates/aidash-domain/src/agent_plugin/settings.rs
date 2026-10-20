//! What an Agent definition decides about the servers of the Agent Plugins it takes in. The pinned
//! plugin copy stays exactly as imported; these choices live beside it.
use super::{AgentPluginRef, McpServer, PluginName, ServerName, reference::valid_digest};
use crate::{Error, Result, exposure::BindingExposure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_SERVER_TOOLS: usize = 64;
pub const MAX_TOOL_NAME: usize = 128;

/// The per-server choices of an Agent definition, keyed by plugin name and server name.
pub type PluginSettings = BTreeMap<PluginName, BTreeMap<ServerName, ServerSettings>>;

/// What an Agent definition decides about one server of an Agent Plugin it takes in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServerSettings {
	/// Allowlist of tool names; `None` offers every tool the server lists. It can only narrow.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tools: Option<BTreeSet<String>>,
	/// Eager or Discoverable under `deferred@1`, the same type and meaning as `Binding.exposure`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub exposure: Option<BindingExposure>,
	/// Tools declared Read-only, each with the inputSchema digest it was declared against.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub read_only: Vec<ReadOnlyTool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadOnlyTool {
	pub name: String,
	pub schema_digest: String,
}

/// 1 to 128 characters without control characters. Settings validation and the Run-start pin share
/// this rule, so a name the settings accept is never refused when the Run pins its tools.
pub(super) fn valid_tool_name(name: &str) -> bool {
	(1..=MAX_TOOL_NAME).contains(&name.chars().count()) && !name.chars().any(char::is_control)
}

impl ServerSettings {
	fn check(&self, remote: bool, deferred: bool) -> std::result::Result<(), &'static str> {
		if self.exposure.is_some() && !deferred {
			return Err("exposure requires the deferred@1 policy");
		}
		if let Some(tools) = &self.tools
			&& (tools.len() > MAX_SERVER_TOOLS || !tools.iter().all(|name| valid_tool_name(name)))
		{
			return Err("the tool allowlist holds at most 64 names of 1 to 128 characters");
		}
		if self.read_only.is_empty() {
			return Ok(());
		}
		if !remote {
			return Err("only a remote server can declare Read-only tools");
		}
		let mut names = BTreeSet::new();
		if self.read_only.len() > MAX_SERVER_TOOLS
			|| !self
				.read_only
				.iter()
				.all(|tool| valid_tool_name(&tool.name) && names.insert(tool.name.as_str()))
		{
			return Err(
				"Read-only declarations hold at most 64 distinct names of 1 to 128 characters",
			);
		}
		if !self
			.read_only
			.iter()
			.all(|tool| valid_digest(&tool.schema_digest))
		{
			return Err("a schema digest is sha256: and 64 lowercase hex digits");
		}
		Ok(())
	}
}

/// Every plugin key names an attached plugin and every server key one of its servers.
pub(crate) fn validate(
	settings: &PluginSettings,
	plugins: &[AgentPluginRef],
	deferred: bool,
) -> Result<()> {
	let invalid = |reason: String| Error::Invalid(format!("AGENT_PLUGIN_SETTINGS: {reason}"));
	for (plugin_name, servers) in settings {
		let plugin = plugins
			.iter()
			.find(|plugin| &plugin.name == plugin_name)
			.ok_or_else(|| invalid(format!("{plugin_name} is not an attached plugin")))?;
		for (server_name, server) in servers {
			let kind = plugin
				.servers
				.get(server_name)
				.ok_or_else(|| invalid(format!("{plugin_name} has no server {server_name}")))?;
			server
				.check(matches!(kind, McpServer::Remote(_)), deferred)
				.map_err(|reason| invalid(format!("{plugin_name}/{server_name}: {reason}")))?;
		}
	}
	Ok(())
}
