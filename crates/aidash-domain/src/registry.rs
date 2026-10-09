//! Portable catalog definitions and immutable installation provenance.
use crate::entities::empty_object;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub type Localized = std::collections::BTreeMap<String, String>;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "MarketplaceProjection")]
#[serde(deny_unknown_fields)]
pub struct Projection {
	pub contract: u8,
	pub tenant: String,
	pub installation: String,
	pub revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
#[schemars(rename = "Entry")]
#[serde(deny_unknown_fields)]
pub struct Entry {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub binding_normalization: Option<bindings::AgentNormalization>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub installation: Option<Projection>,
	pub id: String,
	pub version: String,
	pub kind: String,
	pub name: Localized,
	pub description: Localized,
	#[serde(default)]
	pub capabilities: Vec<String>,
	#[serde(default)]
	pub tags: Vec<String>,
	#[serde(default)]
	pub languages: Vec<String>,
	#[serde(default)]
	pub skills: Vec<String>,
	#[serde(default = "empty_object")]
	pub schema: Value,
	#[serde(default = "empty_object")]
	#[schemars(with = "std::collections::BTreeMap<String, Value>")]
	pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct EntityRef {
	pub id: String,
	pub version: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillFile {
	pub path: String,
	pub content: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub encoding: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Search {
	pub kind: Option<String>,
	pub query: Option<String>,
	pub capability: Option<String>,
	pub language: Option<String>,
	pub skill: Option<String>,
	pub tag: Option<String>,
	pub model: Option<String>,
}

impl SkillFile {
	pub fn byte_len(&self) -> crate::Result<usize> {
		if self.content.len() > 350_000 {
			return Err(crate::Error::Invalid(format!(
				"{} exceeds the file size limit",
				self.path
			)));
		}
		match self.encoding.as_deref() {
			None | Some("utf8") => Ok(self.content.len()),
			Some("base64") => {
				use base64::Engine;
				base64::engine::general_purpose::STANDARD
					.decode(&self.content)
					.map(|bytes| bytes.len())
					.map_err(|_| {
						crate::Error::Invalid(format!("{} has invalid base64 content", self.path))
					})
			}
			_ => Err(crate::Error::Invalid(format!(
				"{} has an unsupported encoding",
				self.path
			))),
		}
	}
}

impl Search {
	pub fn matches(&self, e: &Entry) -> bool {
		self.kind.as_ref().is_none_or(|s| &e.kind == s)
			&& self
				.capability
				.as_ref()
				.is_none_or(|s| e.capabilities.contains(s))
			&& self
				.language
				.as_ref()
				.is_none_or(|s| e.languages.iter().any(|l| l.eq_ignore_ascii_case(s)))
			&& self.skill.as_ref().is_none_or(|s| e.skills.contains(s))
			&& self.tag.as_ref().is_none_or(|s| e.tags.contains(s))
			&& self
				.model
				.as_ref()
				.is_none_or(|s| e.config.pointer("/model/id").and_then(Value::as_str) == Some(s))
			&& self.query.as_ref().is_none_or(|s| {
				let s = s.to_lowercase();
				e.id.to_lowercase().contains(&s)
					|| e.name
						.values()
						.chain(e.description.values())
						.any(|v| v.to_lowercase().contains(&s))
			})
	}
}

/// Runtime settings derived exclusively from a Binding definition or its admitted
/// snapshot. Capability booleans are internal adapter inputs, never writable API
/// fields, and never select the model-visible tool set.
#[derive(Debug, Clone)]
pub struct AgentConfig {
	pub memory: Option<EntityRef>,
	pub sources: Vec<EntityRef>,
	pub conversation_memory: bool,
	pub semantic_memory: bool,
	pub workspace_context: bool,
	pub schema_version: u8,
	pub bindings: Vec<bindings::Binding>,
	pub remove_default: Vec<String>,
	pub model: EntityRef,
	pub instructions: String,
	pub cluster: Option<EntityRef>,
	pub max_steps: i32,
	/// Absent means `legacy@1`; see [`AgentConfig::exposure_policy`].
	pub exposure: Option<crate::exposure::ExposurePolicy>,
	pub core_capabilities: crate::capabilities::CoreCapabilities,
	pub skill_attachments: Vec<crate::capabilities::SkillAttachment>,
	pub skill_roots: Vec<String>,
	pub reference_attachments: Vec<crate::capabilities::ReferenceAttachment>,
	pub knowledge_digest: Option<String>,
	pub tools: Vec<EntityRef>,
	pub skills: Vec<EntityRef>,
	pub allow_task_creation: Option<bool>,
	pub allow_task_delegation: Option<bool>,
	pub allow_memory_write: Option<bool>,
	pub allow_workspace_retrieval: Option<bool>,
	pub allow_cross_conversation_memory: Option<bool>,
}
impl<'de> Deserialize<'de> for AgentConfig {
	fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let input = bindings::AgentBindings::deserialize(deserializer)?;
		input.validate().map_err(serde::de::Error::custom)?;
		Ok(Self::from_definition(input))
	}
}
impl Serialize for AgentConfig {
	fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		self.definition().serialize(serializer)
	}
}
impl JsonSchema for AgentConfig {
	fn schema_name() -> std::borrow::Cow<'static, str> {
		"AgentConfig".into()
	}
	fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
		<bindings::AgentBindings as JsonSchema>::json_schema(generator)
	}
}
mod agent_settings;

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct AgentPage {
	pub entries: Vec<Entry>,
	pub next_offset: Option<u64>,
}

fn max_steps() -> i32 {
	64
}

mod contracts;
pub mod rules;
pub use contracts::*;

pub mod knowledge;

pub mod skill_import;

pub mod workbench;

pub mod bindings;
