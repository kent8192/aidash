//! What an Agent definition decides about the servers of the Agent Plugins it takes in. The pinned
//! plugin copy stays exactly as imported; these choices live beside it.
use super::{AgentPluginRef, McpServer, PluginName, ServerName, reference::valid_digest};
use crate::{Error, Result, exposure::BindingExposure, tool::SecretOwner};
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
	/// The pasted secret a remote server authenticates with when it offers no OAuth (ADR 0100).
	/// A person's OAuth Connection to the server still comes first.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub secret: Option<ServerSecret>,
}

/// The header a [`ServerSecret`] travels in unless the definition names another.
pub const SECRET_HEADER: &str = "Authorization";

fn secret_header() -> String {
	SECRET_HEADER.to_owned()
}

/// How a [`ServerSecret`] goes in its header (ADR 0073, 2026-10-23 and 2026-11-16 amendments).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SecretForm {
	/// `Bearer <value>`. Only `Authorization` carries it: the Credential Broker sends this form
	/// in no other header.
	Bearer,
	/// The value as it is, in any header.
	Raw,
}

/// A Tenant Secret, shared or personal, that one remote server of an Agent Plugin authenticates
/// with (ADR 0073, amended 2026-11-16). The Run's person's personal secret of this name wins over
/// the shared one at dispatch; `owner: personal` refuses the shared one. `Authorization` carries
/// `Bearer <value>` unless `form` is `raw`, and any other header the raw value. It replaces a
/// plugin header of the same name, compared without case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServerSecret {
	/// The Tenant Secret name, as Registry tools write it in `secret`.
	pub name: String,
	#[serde(default = "secret_header")]
	pub header: String,
	/// Without it, `Authorization` takes the Bearer form and any other header the raw one.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub form: Option<SecretForm>,
	#[serde(default, skip_serializing_if = "SecretOwner::is_any")]
	pub owner: SecretOwner,
}

impl ServerSecret {
	/// Whether the value goes as `Bearer <value>`: only in `Authorization`, unless the form is
	/// `raw`. A Bearer form on another header is refused by [`Self::validate`] and never sent.
	pub fn bearer(&self) -> bool {
		self.header.eq_ignore_ascii_case(SECRET_HEADER) && self.form != Some(SecretForm::Raw)
	}

	/// The plugin's literal headers without the one this secret replaces.
	pub fn replace_in(&self, headers: &[super::Header]) -> Vec<super::Header> {
		headers
			.iter()
			.filter(|header| !header.name.eq_ignore_ascii_case(&self.header))
			.cloned()
			.collect()
	}

	/// The name and header rules a definition's `secret` must meet.
	pub fn validate(&self) -> Result<()> {
		self.check()
			.map_err(|reason| Error::Invalid(format!("AGENT_PLUGIN_SETTINGS: {reason}")))
	}

	fn check(&self) -> std::result::Result<(), &'static str> {
		if crate::tenant_secrets::validate_name(&self.name).is_err() {
			return Err("the secret name is not a valid Tenant Secret name");
		}
		if !super::secret_header_valid(&self.header) {
			return Err("the secret header is owned by the transport or refused by the broker");
		}
		if self.form == Some(SecretForm::Bearer) && !self.header.eq_ignore_ascii_case(SECRET_HEADER)
		{
			return Err("only the Authorization header carries the Bearer form");
		}
		Ok(())
	}
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
		if let Some(secret) = &self.secret {
			if !remote {
				return Err("only a remote server authenticates with a secret");
			}
			secret.check()?;
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

/// Each remote server secret the settings name, with the server it authenticates. Registration
/// checks a shared one against the hosts its Tenant Secret records, as for a Registry tool.
pub fn server_secrets<'a>(
	plugins: &'a [AgentPluginRef],
	settings: &'a PluginSettings,
) -> Vec<(&'a ServerSecret, &'a super::RemoteServer)> {
	plugins
		.iter()
		.filter_map(|plugin| Some((plugin, settings.get(&plugin.name)?)))
		.flat_map(|(plugin, servers)| {
			servers.iter().filter_map(|(name, settings)| {
				let McpServer::Remote(remote) = plugin.servers.get(name)? else {
					return None;
				};
				Some((settings.secret.as_ref()?, remote))
			})
		})
		.collect()
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
