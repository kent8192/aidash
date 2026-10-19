//! What an Agent definition records about the plugin copies it takes in, and the Skills a Run pins
//! from them.
use super::{Finding, LoadedPlugin, McpServer, PluginName, PluginSkill, ServerName};
use crate::{
	Error, Result,
	capabilities::{SkillAttachment, skills},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const MAX_AGENT_PLUGINS: usize = 8;
pub const MAX_REFERENCE_SKILLS: usize = 16;
const SOURCE_PREFIX: &str = "https://github.com/";
const SKILL_ORIGIN_LIMIT: usize = 2048;
const SKILL_ORIGIN_SEGMENT: &str = "/skills/";
const MAX_SKILL_DIRECTORY: usize = 64;
/// The longest source for which every Skill origin still fits a direct Skill's origin limit.
pub const MAX_SOURCE_BYTES: usize =
	SKILL_ORIGIN_LIMIT - SKILL_ORIGIN_SEGMENT.len() - MAX_SKILL_DIRECTORY;

/// What an Agent definition records about one Agent Plugin it takes in (ADR 0072).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentPluginRef {
	/// The [`LoadedPlugin::digest`] of the copy.
	pub digest: String,
	pub name: PluginName,
	/// The commit-pinned GitHub URL of the plugin root.
	pub source: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub version: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub license: Option<String>,
	/// The accepted Skills, in directory order.
	pub skills: Vec<String>,
	/// The accepted servers, exactly as the loaded plugin has them.
	pub servers: BTreeMap<ServerName, McpServer>,
	/// Everything ignored or skipped, exactly as the loaded plugin has it.
	pub findings: Vec<Finding>,
}

impl AgentPluginRef {
	/// The one way to build a reference: from a validated plugin and its source.
	pub fn of(plugin: &LoadedPlugin, source: &str) -> Self {
		Self {
			digest: plugin.digest.clone(),
			name: plugin.manifest.name.clone(),
			source: source.to_owned(),
			version: plugin.manifest.version.clone(),
			license: plugin.manifest.license.clone(),
			skills: plugin
				.skills
				.iter()
				.map(|skill| skill.name.clone())
				.collect(),
			servers: plugin.servers.clone(),
			findings: plugin.findings.clone(),
		}
	}

	pub fn validate(&self) -> Result<()> {
		if !valid_digest(&self.digest) {
			return Err(invalid(
				"the digest is not sha256: and 64 lowercase hex digits",
			));
		}
		if !self.source.starts_with(SOURCE_PREFIX) || self.source.len() > MAX_SOURCE_BYTES {
			return Err(invalid(&format!(
				"the source must be a {SOURCE_PREFIX} URL of at most {MAX_SOURCE_BYTES} bytes"
			)));
		}
		if self.skills.len() > MAX_REFERENCE_SKILLS {
			return Err(invalid(&format!(
				"a plugin carries at most {MAX_REFERENCE_SKILLS} Skills"
			)));
		}
		Ok(())
	}
}

fn valid_digest(digest: &str) -> bool {
	digest.strip_prefix("sha256:").is_some_and(|hex| {
		hex.len() == 64
			&& hex
				.bytes()
				.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
	})
}

fn invalid(reason: &str) -> Error {
	Error::Invalid(format!("AGENT_PLUGIN_INVALID: {reason}"))
}

/// The Skills of one Agent Plugin copy, pinned into a Run's Binding snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PinnedAgentPlugin {
	pub digest: String,
	pub skills: Vec<SkillAttachment>,
}

impl PluginSkill {
	/// The direct Skill this plugin Skill becomes. `skill_id` is derived from the plugin digest
	/// and the directory, so every Run of the same copy sees the same id.
	pub fn attachment(&self, plugin_digest: &str, source: &str) -> SkillAttachment {
		let mut attachment = SkillAttachment {
			skill_id: skill_id(plugin_digest, &self.directory),
			origin: format!("{source}{SKILL_ORIGIN_SEGMENT}{}", self.directory),
			digest: String::new(),
			instructions: self.instructions.clone(),
			files: self.files.clone(),
		};
		attachment.digest = skills::content_digest(&attachment);
		attachment
	}
}

/// A version 8 UUID over the first 16 bytes of `sha256(plugin_digest NUL directory)`.
fn skill_id(plugin_digest: &str, directory: &str) -> uuid::Uuid {
	let hash = Sha256::new()
		.chain_update(plugin_digest.as_bytes())
		.chain_update([0])
		.chain_update(directory.as_bytes())
		.finalize();
	let mut bytes = [0; 16];
	bytes.copy_from_slice(&hash[..16]);
	uuid::Builder::from_custom_bytes(bytes).into_uuid()
}
