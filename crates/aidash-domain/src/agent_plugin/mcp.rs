//! `mcp.json`: a file-level problem disables MCP for the plugin, an entry-level problem skips one
//! server, and neither stops the plugin's other components from loading.
use super::{Component, Finding, PluginFile, Reason, SUPPORTED_VERSION, schema_version};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

mod remote;
mod stdio;

pub use remote::{Header, RemoteServer};
pub use stdio::{Command, Cwd, Executable, StdioServer};

#[cfg(test)]
pub(super) use remote::KEYS as REMOTE_KEYS;
#[cfg(test)]
pub(super) use stdio::KEYS as STDIO_KEYS;

const MCP_PATH: &str = "mcp.json";
const MAX_NAME_LEN: usize = 64;

pub(super) const TOP_LEVEL_KEYS: [&str; 2] = ["$schema", "mcpServers"];

/// Aidash's name for a server: 1-64 characters of `A-Za-z0-9_.-`.
#[derive(
	Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(with = "String")]
pub struct ServerName(String);

impl ServerName {
	pub fn as_str(&self) -> &str {
		&self.0
	}
}

fn valid_server_name(name: &str) -> bool {
	!name.is_empty()
		&& name.len() <= MAX_NAME_LEN
		&& name
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

impl TryFrom<String> for ServerName {
	type Error = crate::Error;

	fn try_from(name: String) -> Result<Self, Self::Error> {
		if valid_server_name(&name) {
			Ok(Self(name))
		} else {
			Err(crate::Error::Invalid("INVALID_SERVER_NAME".into()))
		}
	}
}

impl From<ServerName> for String {
	fn from(name: ServerName) -> Self {
		name.0
	}
}

impl fmt::Display for ServerName {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.0)
	}
}

/// `sse` is never represented: a valid legacy entry is reported as an unsupported transport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpServer {
	Stdio(StdioServer),
	Remote(RemoteServer),
}

/// Why one server entry does not load.
struct Refusal {
	reason: Reason,
	detail: String,
}

impl Refusal {
	fn new(reason: Reason, detail: impl Into<String>) -> Self {
		Self {
			reason,
			detail: detail.into(),
		}
	}

	fn invalid(detail: impl Into<String>) -> Self {
		Self::new(Reason::ServerInvalid, detail)
	}
}

enum Entry {
	Server(McpServer),
	LegacySse,
}

pub(super) fn parse(
	files: &[PluginFile],
	tree: &BTreeSet<&str>,
) -> (BTreeMap<ServerName, McpServer>, Vec<Finding>) {
	let Some(file) = files.iter().find(|file| file.path == MCP_PATH) else {
		return Default::default();
	};
	let servers = match entries(&file.bytes) {
		Ok(servers) => servers,
		Err(finding) => return (BTreeMap::new(), vec![finding]),
	};
	let mut loaded = BTreeMap::new();
	let mut findings = Vec::new();
	for (key, value) in servers {
		let component = Component::Server(key.clone());
		let Ok(name) = ServerName::try_from(key.clone()) else {
			findings.push(Finding::skipped(component, Reason::ServerName, key));
			continue;
		};
		match entry(value, tree) {
			Ok(Entry::Server(server)) => {
				loaded.insert(name, server);
			}
			Ok(Entry::LegacySse) => findings.push(Finding::skipped(
				component,
				Reason::UnsupportedTransport,
				"sse",
			)),
			Err(Refusal { reason, detail }) => {
				findings.push(Finding::skipped(component, reason, detail));
			}
		}
	}
	(loaded, findings)
}

/// The `mcpServers` members in key order, or the finding that disables MCP for the plugin.
fn entries(bytes: &[u8]) -> Result<Vec<(String, Value)>, Finding> {
	let disabled = |reason, detail: &str| Finding::skipped(Component::Mcp, reason, detail);
	let invalid = |detail: &str| disabled(Reason::McpInvalid, detail);
	let Ok(Value::Object(mut object)) = serde_json::from_slice::<Value>(bytes) else {
		return Err(invalid("not a JSON object"));
	};
	let Some(Value::String(schema)) = object.get("$schema") else {
		return Err(invalid("$schema must be a string"));
	};
	match schema_version(schema, "mcp.schema.json") {
		Some(SUPPORTED_VERSION) => {}
		Some(version) => return Err(disabled(Reason::McpVersionMismatch, version)),
		None => return Err(invalid("$schema is not an Agent Plugins mcp schema")),
	}
	if let Some(key) = object
		.keys()
		.find(|key| !TOP_LEVEL_KEYS.contains(&key.as_str()))
	{
		return Err(invalid(&format!("unknown field {key}")));
	}
	let Some(Value::Object(servers)) = object.remove("mcpServers") else {
		return Err(invalid("mcpServers must be an object"));
	};
	let mut servers: Vec<(String, Value)> = servers.into_iter().collect();
	servers.sort_by(|a, b| a.0.cmp(&b.0));
	Ok(servers)
}

fn entry(value: Value, tree: &BTreeSet<&str>) -> Result<Entry, Refusal> {
	let Value::Object(object) = value else {
		return Err(Refusal::invalid("server must be an object"));
	};
	match object.get("type").and_then(Value::as_str) {
		Some("stdio") => stdio::parse(&object, tree).map(Entry::Server),
		Some("streamable-http") => remote::parse(&object).map(Entry::Server),
		Some("sse") => remote::parse(&object).map(|_| Entry::LegacySse),
		_ => Err(Refusal::invalid(
			"type must be stdio, streamable-http or sse",
		)),
	}
}

fn closed_keys(object: &Map<String, Value>, allowed: &[&str]) -> Result<(), Refusal> {
	match object.keys().find(|key| !allowed.contains(&key.as_str())) {
		Some(key) => Err(Refusal::invalid(format!("unknown field {key}"))),
		None => Ok(()),
	}
}

fn required_string<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str, Refusal> {
	match object.get(key) {
		Some(Value::String(text)) if !text.is_empty() => Ok(text),
		_ => Err(Refusal::invalid(format!(
			"{key} must be a non-empty string"
		))),
	}
}

fn optional_string_array(object: &Map<String, Value>, key: &str) -> Result<Vec<String>, Refusal> {
	let Some(value) = object.get(key) else {
		return Ok(Vec::new());
	};
	let items = value
		.as_array()
		.ok_or_else(|| Refusal::invalid(format!("{key} must be an array of strings")))?;
	items
		.iter()
		.map(|item| item.as_str().map(str::to_owned))
		.collect::<Option<_>>()
		.ok_or_else(|| Refusal::invalid(format!("{key} must be an array of strings")))
}

fn optional_string_map(
	object: &Map<String, Value>,
	key: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
	let Some(value) = object.get(key) else {
		return Ok(BTreeMap::new());
	};
	value
		.as_object()
		.and_then(|members| {
			members
				.iter()
				.map(|(name, member)| Some((name.clone(), member.as_str()?.to_owned())))
				.collect()
		})
		.ok_or_else(|| Refusal::invalid(format!("{key} must be an object of strings")))
}
