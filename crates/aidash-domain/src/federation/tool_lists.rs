//! Federation Tool Request, Federation Tool Allowance and Federation Tool Block (ADR 0139).
//!
//! A Run that a Remote Grant started may use a listable tool only when both the Home's
//! Request and the receiving Node's Allowance name it, and the receiver's Block does not.
//! A tool that either side lacks is not refused outright: the Run stops and asks that side.
use crate::tool::ToolConfig;
use crate::tool::contract::{ToolContract, ToolIdentity, ToolReach, builtin_reach};
use crate::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Most patterns one list may hold.
pub const MAX_PATTERNS: usize = 256;
/// Longest pattern, in bytes.
pub const MAX_PATTERN_BYTES: usize = 200;
/// The `type` of the RFC 9396 `authorization_details` entry a Remote Grant carries.
pub const AUTHORIZATION_DETAIL_TYPE: &str = "aidash.federation.tools";

/// The tools a new Node Registration starts with on both sides: those acting only on the
/// Home's own Workspace and people, plus the receiving Agent's Skills.
pub const DEFAULT_TOOLS: &[&str] = &[
	"task_create",
	"artifact_publish",
	"workspace_message",
	"workspace_read",
	"workspace_observe",
	"memory_recall",
	"memory_reflect",
	"memory_mutate",
	"human_request",
	"skill_read",
	"skill_asset_read",
];

/// What a call to a tool that is blocked, Reach never, or unknown to this Node answers a remote
/// Run, the same words for all three so the answer reveals nothing about which it was.
pub const UNAVAILABLE_TOOL: &str = "this tool is not available on this Node";

/// The key a list matches for a Registry tool.
pub fn registry_key(definition: &str, tool: &str) -> String {
	format!("registry:{definition}/{tool}")
}

/// The operation a Registry tool definition is listed under: the MCP server's own tool name, the
/// native operation, or `invoke` for the one operation of an HTTP or Agent tool.
pub fn registry_operation(config: &ToolConfig) -> &str {
	match config {
		ToolConfig::Mcp { tool_name, .. } => tool_name,
		ToolConfig::Native { operation, .. } => operation,
		ToolConfig::Http { .. } | ToolConfig::Agent { .. } => "invoke",
	}
}

/// The key the Federation tool lists match for `contract`. A builtin, and a core provider's
/// descriptor, is its builtin name; a Registry tool or a descriptor of another provider is
/// `registry:<definition id>/<operation>`, which names the definition and operation but not
/// the version, so a list outlives a new version. `operation` is the descriptor's operation or
/// [`registry_operation`]; it is ignored for a builtin.
pub fn list_key(contract: &ToolContract, operation: &str) -> String {
	match &contract.identity {
		ToolIdentity::Builtin(name) => name.strip_prefix("builtin:").unwrap_or(name).to_owned(),
		ToolIdentity::Registry(reference) => registry_key(&reference.id, operation),
		ToolIdentity::Descriptor(_) if builtin_reach(operation).is_some() => operation.to_owned(),
		ToolIdentity::Descriptor(reference) => registry_key(&reference.id, operation),
		// Never reaches a remote Run, so no list names it.
		ToolIdentity::AgentPlugin(tool) => {
			format!("agent-plugin:{}/{}/{}", tool.plugin, tool.server, tool.tool)
		}
	}
}

/// The key a Registry definition `id` is listed under, read from its `config` the way [`list_key`]
/// reads a contract: a core provider's descriptor is its builtin name, any other descriptor or
/// a tool transport `registry:<id>/<operation>`. `None` when `config` is neither.
pub fn definition_key(id: &str, config: &serde_json::Value) -> Option<String> {
	if config.get("provider").is_some() {
		let descriptor: crate::tool::providers::ToolDescriptor =
			serde_json::from_value(config.clone()).ok()?;
		return Some(if builtin_reach(&descriptor.operation).is_some() {
			descriptor.operation
		} else {
			registry_key(id, &descriptor.operation)
		});
	}
	let config: ToolConfig = serde_json::from_value(config.clone()).ok()?;
	Some(registry_key(id, registry_operation(&config)))
}

/// One tool a receiving Node offers a peer for its Request, as `scoped/tools/discover` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiscoveredTool {
	pub key: String,
	pub reach: ToolReach,
}

/// The answer of `scoped/tools/discover`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolDiscovery {
	pub tools: Vec<DiscoveredTool>,
}

/// One tool name or `*` pattern, such as `file_*` or `registry:github/*`.
#[derive(
	Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct ToolPattern(String);

impl ToolPattern {
	pub fn new(pattern: impl Into<String>) -> Result<Self> {
		let pattern = pattern.into();
		let valid = |c: char| {
			c.is_ascii_lowercase()
				|| c.is_ascii_digit()
				|| matches!(c, '_' | '-' | '.' | ':' | '/' | '@' | '*')
		};
		if pattern.is_empty() || pattern.len() > MAX_PATTERN_BYTES || !pattern.chars().all(valid) {
			return Err(Error::Invalid(format!(
				"tool pattern must be 1 to {MAX_PATTERN_BYTES} lowercase letters, digits or _-.:/@*"
			)));
		}
		Ok(Self(pattern))
	}

	pub fn as_str(&self) -> &str {
		&self.0
	}

	/// Whether `key` matches, where `*` stands for any run of characters, `/` included.
	pub fn matches(&self, key: &str) -> bool {
		glob(self.0.as_bytes(), key.as_bytes())
	}
}

impl TryFrom<String> for ToolPattern {
	type Error = Error;
	fn try_from(value: String) -> Result<Self> {
		Self::new(value)
	}
}

impl From<ToolPattern> for String {
	fn from(value: ToolPattern) -> Self {
		value.0
	}
}

/// Iterative `*` matching with single backtracking, linear in practice and never recursive,
/// so a hostile pattern cannot blow the stack.
fn glob(pattern: &[u8], text: &[u8]) -> bool {
	let (mut p, mut t) = (0, 0);
	let mut star: Option<(usize, usize)> = None;
	while t < text.len() {
		if p < pattern.len() && pattern[p] == b'*' {
			star = Some((p, t));
			p += 1;
		} else if p < pattern.len() && pattern[p] == text[t] {
			p += 1;
			t += 1;
		} else if let Some((star_p, star_t)) = star {
			p = star_p + 1;
			t = star_t + 1;
			star = Some((star_p, star_t + 1));
		} else {
			return false;
		}
	}
	pattern[p..].iter().all(|&c| c == b'*')
}

/// A Federation Tool Request, Allowance or Block: sorted, without duplicates.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "Vec<ToolPattern>", into = "Vec<ToolPattern>")]
pub struct ToolList(Vec<ToolPattern>);

impl ToolList {
	pub fn new(patterns: impl IntoIterator<Item = ToolPattern>) -> Result<Self> {
		let mut patterns: Vec<_> = patterns.into_iter().collect();
		patterns.sort();
		patterns.dedup();
		if patterns.len() > MAX_PATTERNS {
			return Err(Error::Invalid(format!(
				"a tool list holds at most {MAX_PATTERNS} patterns"
			)));
		}
		Ok(Self(patterns))
	}

	pub fn defaults() -> Self {
		Self::new(
			DEFAULT_TOOLS
				.iter()
				.map(|name| ToolPattern((*name).to_owned())),
		)
		.expect("default tools are valid patterns")
	}

	pub fn patterns(&self) -> &[ToolPattern] {
		&self.0
	}

	pub fn matches(&self, key: &str) -> bool {
		self.0.iter().any(|pattern| pattern.matches(key))
	}

	/// The list with `pattern` added, as an answer "add to the list" does.
	pub fn with(&self, pattern: ToolPattern) -> Result<Self> {
		Self::new(self.0.iter().cloned().chain([pattern]))
	}
}

impl TryFrom<Vec<ToolPattern>> for ToolList {
	type Error = Error;
	fn try_from(value: Vec<ToolPattern>) -> Result<Self> {
		Self::new(value)
	}
}

impl From<ToolList> for Vec<ToolPattern> {
	fn from(value: ToolList) -> Self {
		value.0
	}
}

/// One egress host or `*` host pattern a Home lets its remote Runs' commands reach.
#[derive(
	Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct HostPattern(String);

impl HostPattern {
	pub fn new(pattern: impl Into<String>) -> Result<Self> {
		let pattern = pattern.into();
		let valid =
			|c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.' | '*');
		if pattern.is_empty() || pattern.len() > 253 || !pattern.chars().all(valid) {
			return Err(Error::Invalid(
				"host pattern must be 1 to 253 lowercase letters, digits or -.*".into(),
			));
		}
		Ok(Self(pattern))
	}

	pub fn as_str(&self) -> &str {
		&self.0
	}

	pub fn matches(&self, host: &str) -> bool {
		glob(self.0.as_bytes(), host.to_ascii_lowercase().as_bytes())
	}
}

impl TryFrom<String> for HostPattern {
	type Error = Error;
	fn try_from(value: String) -> Result<Self> {
		Self::new(value)
	}
}

impl From<HostPattern> for String {
	fn from(value: HostPattern) -> Self {
		value.0
	}
}

/// What a Home asks a receiving Node for, as carried by each Remote Grant in the shape of an
/// RFC 9396 `authorization_details` entry: `actions` are tool patterns, `locations` egress hosts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolAuthorizationDetail {
	#[serde(rename = "type")]
	pub kind: String,
	pub actions: ToolList,
	#[serde(default)]
	pub locations: Vec<HostPattern>,
}

impl ToolAuthorizationDetail {
	pub fn new(request: &FederationToolRequest) -> Self {
		Self {
			kind: AUTHORIZATION_DETAIL_TYPE.into(),
			actions: request.tools.clone(),
			locations: request.hosts.clone(),
		}
	}

	pub fn request(&self) -> Result<FederationToolRequest> {
		if self.kind != AUTHORIZATION_DETAIL_TYPE {
			return Err(Error::Invalid(format!(
				"unsupported authorization detail type {}",
				self.kind
			)));
		}
		Ok(FederationToolRequest {
			tools: self.actions.clone(),
			hosts: self.locations.clone(),
		})
	}
}

/// The tools and egress hosts one Node asks another Node to let its remote Runs use.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FederationToolRequest {
	pub tools: ToolList,
	#[serde(default)]
	pub hosts: Vec<HostPattern>,
}

impl FederationToolRequest {
	pub fn defaults() -> Self {
		Self {
			tools: ToolList::defaults(),
			hosts: Vec::new(),
		}
	}

	/// Whether a command may reach `host`: the Home must name it as well as the receiver's
	/// Scratch Space settings.
	pub fn allows_host(&self, host: &str) -> bool {
		self.hosts.iter().any(|pattern| pattern.matches(host))
	}
}

/// Which side lacks a tool a remote Run called.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Lacking {
	pub home: bool,
	pub receiver: bool,
}

/// What a remote Run's call to one tool may do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolVerdict {
	Usable,
	/// Stop the Run and ask the side, or both sides, that lack the tool.
	Ask(Lacking),
	/// Not offered: Reach never, or blocked. Looks the same as a tool that does not exist.
	Unavailable,
}

/// Everything that decides one remote Run's tools between Nodes of different Tenants.
#[derive(Clone, Copy, Debug)]
pub struct RemoteToolLists<'a> {
	pub request: &'a ToolList,
	pub allowance: &'a ToolList,
	pub block: &'a ToolList,
	/// Tools the Home's people allowed for this Run only.
	pub run_request: &'a ToolList,
	/// Tools the receiver's people allowed for this Run only.
	pub run_allowance: &'a ToolList,
}

/// [`RemoteToolLists`] that owns its lists, as a port hands them over.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnedRemoteToolLists {
	pub request: ToolList,
	pub allowance: ToolList,
	pub block: ToolList,
	pub run_request: ToolList,
	pub run_allowance: ToolList,
}

impl OwnedRemoteToolLists {
	pub fn lists(&self) -> RemoteToolLists<'_> {
		RemoteToolLists {
			request: &self.request,
			allowance: &self.allowance,
			block: &self.block,
			run_request: &self.run_request,
			run_allowance: &self.run_allowance,
		}
	}
}

impl RemoteToolLists<'_> {
	pub fn decide(&self, reach: ToolReach, key: &str) -> ToolVerdict {
		match reach {
			ToolReach::Never => return ToolVerdict::Unavailable,
			ToolReach::Run => return ToolVerdict::Usable,
			_ => {}
		}
		if self.block.matches(key) {
			return ToolVerdict::Unavailable;
		}
		let home = self.request.matches(key) || self.run_request.matches(key);
		let receiver = self.allowance.matches(key) || self.run_allowance.matches(key);
		if home && receiver {
			ToolVerdict::Usable
		} else {
			ToolVerdict::Ask(Lacking {
				home: !home,
				receiver: !receiver,
			})
		}
	}

	/// Whether the receiver lets the peer see `key` through `scoped/tools/discover`.
	pub fn discoverable(allowance: &ToolList, block: &ToolList, key: &str) -> bool {
		allowance.matches(key) && !block.matches(key)
	}
}

#[cfg(test)]
mod tests;
