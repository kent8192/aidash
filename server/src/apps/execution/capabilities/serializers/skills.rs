use serde::{Deserialize, Serialize};
// Serializable skills contracts.
use crate::apps::execution::capabilities::services::core::contracts::*;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub(crate) struct Pinned {
	#[serde(default)]
	pub(crate) loaded: bool,
	pub(crate) metadata: SkillMetadata,
	pub(crate) files: Vec<FileEntry>,
	/// Escaped SKILL.md length when embedded in a JSON system message.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub(crate) instruction_json_len: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct SkillList {
	pub cursor: Option<usize>,
	pub limit: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct SkillLoad {
	pub skill_id: Uuid,
	pub expected_digest: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct SkillRead {
	pub skill_id: Uuid,
	pub digest: String,
	pub path: String,
	/// Zero-based Unicode scalar-value position in the text file.
	pub offset: Option<usize>,
	/// Maximum scalar values; encoded-byte resource limits apply independently.
	pub max_chars: Option<usize>,
}

use uuid::Uuid;

pub use aidash_domain::capabilities::SkillAttachment;

pub use aidash_domain::capabilities::skills::SkillMetadata;
