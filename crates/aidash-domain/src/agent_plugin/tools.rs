//! The tools a Run pins at its start from the remote MCP servers of its Agent's plugins. [`pin`]
//! is pure: the application lists each server and hands over what came back.
use super::{
	AgentPluginRef, Component, Finding, McpServer, Outcome, PluginName, PluginSettings, Reason,
	ServerName, ServerSettings, model_alias,
	settings::{MAX_SERVER_TOOLS, MAX_TOOL_NAME, valid_tool_name},
};
use crate::exposure::BindingExposure;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_RUN_TOOLS: usize = 128;
pub const MAX_SCHEMA_BYTES: usize = 16_384;
pub const MAX_DESCRIPTION_BYTES: usize = 8_192;

/// A tool as an MCP server listed it (`tools/list`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListedTool {
	pub name: String,
	pub description: Option<String>,
	pub input_schema: Value,
}

/// One tool a Run pinned at its start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PinnedTool {
	/// The server's own name for the tool.
	pub name: String,
	/// What the model calls it: [`model_alias`] of the plugin, the server and the name.
	pub alias: String,
	pub description: String,
	pub input_schema: Value,
	/// [`schema_digest`] of `input_schema`.
	pub schema_digest: String,
	/// Declared Read-only against exactly this `schema_digest`.
	pub read_only: bool,
	/// The server's settings choose eager exposure.
	pub eager: bool,
}

/// The tools one remote server contributed, tied to the plugin copy that declared it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PinnedServer {
	pub plugin: PluginName,
	/// The [`AgentPluginRef::digest`] of the copy.
	pub digest: String,
	pub server: ServerName,
	pub tools: Vec<PinnedTool>,
}

/// A Run-start finding names its plugin, because [`Finding::component`] only names the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginFinding {
	pub plugin: PluginName,
	pub finding: Finding,
}

/// What a Run pinned at its start: the servers it reached and everything left out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginTools {
	pub servers: Vec<PinnedServer>,
	pub findings: Vec<PluginFinding>,
}

/// What listing one server produced, as the application hands it over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Listing {
	Listed(Vec<ListedTool>),
	/// A typed cause code, never a URL or a response body.
	Unavailable(String),
}

pub fn schema_digest(schema: &Value) -> String {
	crate::registry::rules::digest(schema)
}

/// Decides the Run's plugin tools from what each remote server listed, in plugin order, then
/// server key order, then tool name order. Stdio servers are not pinned here. `taken` holds the
/// aliases the Run's Bindings and the reserved names already use.
pub fn pin(
	plugins: &[AgentPluginRef],
	settings: &PluginSettings,
	listed: &BTreeMap<(PluginName, ServerName), Listing>,
	taken: &BTreeSet<String>,
) -> PluginTools {
	let mut pinner = Pinner {
		taken,
		aliases: BTreeSet::new(),
		count: 0,
		pinned: PluginTools::default(),
	};
	let unchosen = ServerSettings::default();
	for plugin in plugins {
		for (server, kind) in &plugin.servers {
			if !matches!(kind, McpServer::Remote(_)) {
				continue;
			}
			let choice = settings
				.get(&plugin.name)
				.and_then(|servers| servers.get(server))
				.unwrap_or(&unchosen);
			match listed.get(&(plugin.name.clone(), server.clone())) {
				Some(Listing::Listed(tools)) => pinner.server(plugin, server, choice, tools),
				Some(Listing::Unavailable(cause)) => pinner.note(
					plugin,
					server,
					Outcome::Skipped,
					Reason::ServerUnavailable,
					cause.clone(),
				),
				None => pinner.note(
					plugin,
					server,
					Outcome::Skipped,
					Reason::ServerUnavailable,
					"not listed".into(),
				),
			}
		}
	}
	pinner.pinned
}

struct Pinner<'a> {
	taken: &'a BTreeSet<String>,
	/// Aliases given to plugin tools of this Run so far.
	aliases: BTreeSet<String>,
	count: usize,
	pinned: PluginTools,
}

impl Pinner<'_> {
	fn note(
		&mut self,
		plugin: &AgentPluginRef,
		server: &ServerName,
		outcome: Outcome,
		reason: Reason,
		detail: String,
	) {
		let component = Component::Server(server.to_string());
		let finding = match outcome {
			Outcome::Ignored => Finding::ignored(component, reason, detail),
			Outcome::Skipped => Finding::skipped(component, reason, detail),
		};
		self.pinned.findings.push(PluginFinding {
			plugin: plugin.name.clone(),
			finding,
		});
	}

	fn server(
		&mut self,
		plugin: &AgentPluginRef,
		server: &ServerName,
		choice: &ServerSettings,
		listed: &[ListedTool],
	) {
		let mut tools: Vec<&ListedTool> = listed.iter().collect();
		tools.sort_by(|a, b| a.name.cmp(&b.name));
		if let Some(allowed) = &choice.tools {
			for wanted in allowed {
				if !tools.iter().any(|tool| &tool.name == wanted) {
					self.note(
						plugin,
						server,
						Outcome::Ignored,
						Reason::ToolNotListed,
						label(wanted),
					);
				}
			}
			tools.retain(|tool| allowed.contains(&tool.name));
		}
		let eager = choice.exposure == Some(BindingExposure::Eager);
		let mut pinned = Vec::new();
		for tool in tools {
			if let Some(why) = refusal(tool) {
				let detail = format!("{}: {why}", label(&tool.name));
				self.note(
					plugin,
					server,
					Outcome::Skipped,
					Reason::ToolInvalid,
					detail,
				);
				continue;
			}
			let alias = model_alias(&plugin.name, server, &tool.name);
			let skipped = if self.taken.contains(&alias) {
				Some(Reason::ReservedAlias)
			} else if self.aliases.contains(&alias) {
				Some(Reason::AliasCollision)
			} else if pinned.len() >= MAX_SERVER_TOOLS || self.count >= MAX_RUN_TOOLS {
				Some(Reason::ToolLimit)
			} else {
				None
			};
			if let Some(reason) = skipped {
				self.note(plugin, server, Outcome::Skipped, reason, label(&tool.name));
				continue;
			}
			self.aliases.insert(alias.clone());
			self.count += 1;
			let digest = schema_digest(&tool.input_schema);
			let read_only = match choice.read_only.iter().find(|d| d.name == tool.name) {
				Some(declared) if declared.schema_digest == digest => true,
				Some(_) => {
					self.note(
						plugin,
						server,
						Outcome::Ignored,
						Reason::ReadOnlyChanged,
						label(&tool.name),
					);
					false
				}
				None => false,
			};
			pinned.push(PinnedTool {
				name: tool.name.clone(),
				alias,
				description: tool.description.clone().unwrap_or_default(),
				input_schema: tool.input_schema.clone(),
				schema_digest: digest,
				read_only,
				eager,
			});
		}
		self.pinned.servers.push(PinnedServer {
			plugin: plugin.name.clone(),
			digest: plugin.digest.clone(),
			server: server.clone(),
			tools: pinned,
		});
	}
}

/// Why a listed tool cannot be pinned, if it cannot.
fn refusal(tool: &ListedTool) -> Option<&'static str> {
	if !valid_tool_name(&tool.name) {
		Some("name")
	} else if !tool.input_schema.is_object() {
		Some("schema_not_object")
	} else if tool.input_schema.to_string().len() > MAX_SCHEMA_BYTES {
		Some("schema_too_large")
	} else if tool
		.description
		.as_ref()
		.is_some_and(|description| description.len() > MAX_DESCRIPTION_BYTES)
	{
		Some("description_too_large")
	} else {
		None
	}
}

/// A tool name as a finding shows it: bounded, without control characters.
fn label(name: &str) -> String {
	name.chars()
		.take(MAX_TOOL_NAME)
		.map(|c| if c.is_control() { '\u{fffd}' } else { c })
		.collect()
}
