use super::{Refusal, closed_keys, optional_string_array, optional_string_map, required_string};
use crate::agent_plugin::{Reason, paths};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::agent_plugin) const KEYS: [&str; 5] = ["type", "command", "args", "env", "cwd"];

const RESERVED_ENV: [&str; 2] = ["PLUGIN_ROOT", "PLUGIN_DATA"];
const SHELL_CHARS: &str = ";&|<>`$()'\"\\";
const PLUGIN_ROOT: &str = "${PLUGIN_ROOT}";
const PLUGIN_DATA: &str = "${PLUGIN_DATA}";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StdioServer {
	pub command: Command,
	#[serde(default)]
	pub args: Vec<String>,
	#[serde(default)]
	pub env: BTreeMap<String, String>,
	#[serde(default)]
	pub cwd: Cwd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Command {
	Bare(Executable),
	/// The normalized path in the plugin tree, without the leading `./`.
	PluginRelative(String),
}

/// The bare commands the runner image of ADR 0071 provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Executable {
	Node,
	Npm,
	Npx,
	Python3,
	Uv,
	Uvx,
}

impl Executable {
	fn from_name(name: &str) -> Option<Self> {
		Some(match name {
			"node" => Self::Node,
			"npm" => Self::Npm,
			"npx" => Self::Npx,
			"python3" => Self::Python3,
			"uv" => Self::Uv,
			"uvx" => Self::Uvx,
			_ => return None,
		})
	}
}

/// Working directories hold normalized paths relative to their base.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Cwd {
	#[default]
	PluginRoot,
	Plugin(String),
	/// A path inside `${PLUGIN_DATA}`; empty names the data directory itself.
	Data(String),
}

pub(super) fn parse(
	object: &Map<String, Value>,
	tree: &BTreeSet<&str>,
) -> Result<super::McpServer, Refusal> {
	closed_keys(object, &KEYS)?;
	let command = required_string(object, "command")?;
	let args = optional_string_array(object, "args")?;
	let env = optional_string_map(object, "env")?;
	if let Some(name) = RESERVED_ENV.iter().find(|name| env.contains_key(**name)) {
		return Err(Refusal::invalid(format!("env must not set {name}")));
	}
	let cwd = match object.get("cwd") {
		None => Cwd::PluginRoot,
		Some(Value::String(cwd)) => parse_cwd(cwd)?,
		Some(_) => return Err(Refusal::invalid("cwd must be a string")),
	};
	let command = parse_command(command, tree)?;
	Ok(super::McpServer::Stdio(StdioServer {
		command,
		args,
		env,
		cwd,
	}))
}

/// A single executable token: a plugin-relative file or one of Aidash's bare commands.
fn parse_command(command: &str, tree: &BTreeSet<&str>) -> Result<Command, Refusal> {
	if command
		.chars()
		.any(|c| c.is_whitespace() || c.is_control() || SHELL_CHARS.contains(c))
	{
		return Err(Refusal::invalid(
			"command must be a single executable token, not a shell string",
		));
	}
	if let Some(relative) = command.strip_prefix("./") {
		if relative.ends_with('/') {
			return Err(Refusal::invalid("command must name a file"));
		}
		let path = paths::normalize(relative)
			.ok_or_else(|| Refusal::new(Reason::PathOutsideRoot, command))?;
		return if tree.contains(path.as_str()) {
			Ok(Command::PluginRelative(path))
		} else {
			Err(Refusal::new(Reason::MissingCommand, command))
		};
	}
	if command.contains('/') {
		return Err(Refusal::invalid(
			"command must be a bare executable name or start with ./",
		));
	}
	Executable::from_name(command)
		.map(Command::Bare)
		.ok_or_else(|| Refusal::new(Reason::UnsupportedCommand, command))
}

/// `./`, `${PLUGIN_ROOT}` or `${PLUGIN_DATA}` followed by a path that stays inside its base.
fn parse_cwd(cwd: &str) -> Result<Cwd, Refusal> {
	let (data, relative) = if let Some(relative) = cwd.strip_prefix("./") {
		(false, relative)
	} else if let Some(relative) = after_placeholder(cwd, PLUGIN_ROOT) {
		(false, relative)
	} else if let Some(relative) = after_placeholder(cwd, PLUGIN_DATA) {
		(true, relative)
	} else {
		return Err(Refusal::invalid(
			"cwd must start with ./, ${PLUGIN_ROOT} or ${PLUGIN_DATA}",
		));
	};
	if relative.contains('\\') || relative.chars().any(char::is_control) {
		return Err(Refusal::invalid(
			"cwd must not contain a backslash or control character",
		));
	}
	let path =
		paths::normalize(relative).ok_or_else(|| Refusal::new(Reason::PathOutsideRoot, cwd))?;
	Ok(match (data, path.is_empty()) {
		(true, _) => Cwd::Data(path),
		(false, true) => Cwd::PluginRoot,
		(false, false) => Cwd::Plugin(path),
	})
}

/// What follows a placeholder that ends the string or a path segment.
fn after_placeholder<'a>(cwd: &'a str, placeholder: &str) -> Option<&'a str> {
	let rest = cwd.strip_prefix(placeholder)?;
	if rest.is_empty() {
		Some(rest)
	} else {
		rest.strip_prefix('/')
	}
}
