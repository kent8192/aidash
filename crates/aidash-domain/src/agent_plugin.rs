//! Parser and validator for Agent Plugins 1.0.0 directories (ADR 0072).
//!
//! The fetching layer hands over the files of one plugin directory. [`load`] decides what is
//! accepted, skipped or refused; the vendored schemas under `schemas/1.0.0/` are never fetched.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod alias;
mod expand;
#[cfg(test)]
pub(crate) mod fixtures;
mod manifest;
mod mcp;
mod paths;
mod reference;
mod skills;

pub use alias::model_alias;
pub use expand::expand;
pub use manifest::{Author, Manifest, PluginName};
pub use mcp::{Command, Cwd, Executable, Header, McpServer, RemoteServer, ServerName, StdioServer};
pub use paths::tree_digest;
pub use reference::{
	AgentPluginRef, MAX_AGENT_PLUGINS, MAX_REFERENCE_SKILLS, MAX_SOURCE_BYTES, PinnedAgentPlugin,
};
pub use skills::PluginSkill;

pub const PLUGIN_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
pub const MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

const SCHEMA_PREFIX: &str = "https://agent-plugins.org/schemas/";
const SUPPORTED_VERSION: &str = "1.0.0";

/// One file of a plugin directory as fetched, relative to the plugin root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginFile {
	pub path: String,
	pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LoadedPlugin {
	pub manifest: Manifest,
	/// Sorted by directory.
	pub skills: Vec<PluginSkill>,
	pub servers: BTreeMap<ServerName, McpServer>,
	/// Manifest findings first, then `mcp.json` findings by server key, then Skills by directory.
	pub findings: Vec<Finding>,
	/// [`tree_digest`] of the input files.
	pub digest: String,
}

/// A part of a plugin that was ignored or skipped, reported next to the accepted ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Finding {
	pub component: Component,
	pub outcome: Outcome,
	pub reason: Reason,
	pub detail: String,
}

impl Finding {
	fn ignored(component: Component, reason: Reason, detail: impl Into<String>) -> Self {
		Self {
			component,
			outcome: Outcome::Ignored,
			reason,
			detail: detail.into(),
		}
	}

	fn skipped(component: Component, reason: Reason, detail: impl Into<String>) -> Self {
		Self {
			component,
			outcome: Outcome::Skipped,
			reason,
			detail: detail.into(),
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Component {
	Manifest,
	Mcp,
	Skill(String),
	Server(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
	/// The plugin loads; the offending part has no effect.
	Ignored,
	/// The component is left out; the rest of the plugin loads.
	Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
	UnknownField,
	ExtensionsNotObject,
	McpInvalid,
	McpVersionMismatch,
	ServerName,
	ServerInvalid,
	UnsupportedTransport,
	UnsupportedCommand,
	MissingCommand,
	PathOutsideRoot,
	SkillInvalid,
	SkillLimit,
}

/// Fatal outcomes: the plugin is refused and none of its components run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum Rejection {
	#[error("plugin.json is missing")]
	NoManifest,
	#[error("plugin.json is not a JSON object: {0}")]
	InvalidManifest(String),
	#[error("Agent Plugins version {0} is not supported")]
	UnsupportedVersion(String),
	#[error("plugin.json violates its schema at {0}")]
	SchemaViolation(String),
	#[error("invalid path {0:?}")]
	InvalidPath(String),
	#[error("the plugin exceeds the file count or size limit")]
	PackageLimit,
}

impl From<Rejection> for crate::Error {
	fn from(rejection: Rejection) -> Self {
		let (code, detail) = match rejection {
			Rejection::NoManifest => ("AGENT_PLUGIN_NO_MANIFEST", None),
			Rejection::InvalidManifest(detail) => ("AGENT_PLUGIN_INVALID_MANIFEST", Some(detail)),
			Rejection::UnsupportedVersion(detail) => {
				("AGENT_PLUGIN_UNSUPPORTED_VERSION", Some(detail))
			}
			Rejection::SchemaViolation(detail) => ("AGENT_PLUGIN_SCHEMA_VIOLATION", Some(detail)),
			Rejection::InvalidPath(detail) => ("AGENT_PLUGIN_INVALID_PATH", Some(detail)),
			Rejection::PackageLimit => ("AGENT_PLUGIN_PACKAGE_LIMIT", None),
		};
		Self::Invalid(match detail {
			Some(detail) => format!("{code}: {detail}"),
			None => code.to_owned(),
		})
	}
}

/// Parses and validates one plugin directory.
///
/// Limits and path rules are checked before any file is parsed. A manifest problem refuses the
/// plugin; a problem in `mcp.json`, a server entry or a Skill only skips that part.
pub fn load(files: &[PluginFile]) -> Result<LoadedPlugin, Rejection> {
	paths::check_tree(files)?;
	let (manifest, mut findings) = manifest::parse(files)?;
	let tree: BTreeSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
	let (servers, mcp_findings) = mcp::parse(files, &tree);
	let (skills, skill_findings) = skills::discover(files);
	findings.extend(mcp_findings);
	findings.extend(skill_findings);
	Ok(LoadedPlugin {
		manifest,
		skills,
		servers,
		findings,
		digest: tree_digest(files),
	})
}

/// The `<v>` of `https://agent-plugins.org/schemas/<v>/<document>`.
fn schema_version<'a>(schema: &'a str, document: &str) -> Option<&'a str> {
	let version = schema
		.strip_prefix(SCHEMA_PREFIX)?
		.strip_suffix(document)?
		.strip_suffix('/')?;
	(!version.is_empty() && !version.contains('/')).then_some(version)
}

#[cfg(test)]
mod tests;
