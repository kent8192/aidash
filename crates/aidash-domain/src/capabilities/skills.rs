//! Validation for immutable Skill packages and mounted discovery roots.
use super::SkillAttachment;
use crate::{Error, Result, registry::AgentConfig};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SkillMetadata {
	pub skill_id: Uuid,
	pub name: String,
	pub description: String,
	pub origin: String,
	pub digest: String,
	pub license: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct Frontmatter {
	name: String,
	description: String,
	license: Option<String>,
}

fn metadata(attachment: &SkillAttachment) -> Result<SkillMetadata> {
	let text = attachment.instructions.replace("\r\n", "\n");
	let front = text
		.strip_prefix("---\n")
		.and_then(|t| t.split_once("\n---"))
		.map(|p| p.0)
		.ok_or_else(|| Error::Invalid("SKILL_FRONTMATTER_REQUIRED".into()))?;
	if front.len() > 8192 {
		return Err(Error::Invalid("SKILL_METADATA_LIMIT".into()));
	}
	let meta: Frontmatter = serde_saphyr::from_str(front)
		.map_err(|_| Error::Invalid("INVALID_SKILL_METADATA".into()))?;
	if meta.name.is_empty()
		|| meta.name.len() > 128
		|| meta.description.is_empty()
		|| meta.description.len() > 2048
		|| meta.license.as_ref().is_some_and(|s| s.len() > 2048)
	{
		return Err(Error::Invalid("SKILL_METADATA_LIMIT".into()));
	}
	Ok(SkillMetadata {
		skill_id: attachment.skill_id,
		name: meta.name,
		description: meta.description,
		license: meta.license,
		origin: attachment.origin.clone(),
		digest: attachment.digest.clone(),
	})
}

pub fn content_digest(a: &SkillAttachment) -> String {
	crate::registry::rules::digest(&json!({"instructions":a.instructions,"files":a.files}))
}

pub fn validate(a: &SkillAttachment) -> Result<SkillMetadata> {
	if a.origin.is_empty()
		|| a.origin.len() > 2048
		|| a.instructions.len() > 65536
		|| a.instructions.contains('\0')
		|| a.files.len() + 1 > 64
	{
		return Err(Error::Invalid("SKILL_PACKAGE_LIMIT".into()));
	}
	let mut total = a.instructions.len();
	let mut paths = BTreeSet::from(["SKILL.md".to_owned()]);
	for file in &a.files {
		if !crate::registry::rules::valid_skill_file_path(&file.path)
			|| !paths.insert(file.path.clone())
			|| file.content.contains('\0')
		{
			return Err(Error::Invalid("INVALID_SKILL_PATH".into()));
		}
		total += file.byte_len()?;
	}
	if total > 256_000 {
		return Err(Error::Invalid("SKILL_PACKAGE_LIMIT".into()));
	}
	if a.digest != content_digest(a) {
		return Err(Error::Conflict("SKILL_CONTENT_CHANGED".into()));
	}
	metadata(a)
}

pub fn validate_config(config: &AgentConfig) -> Result<()> {
	if config.skill_attachments.len() > 16 || config.skill_roots.len() > 8 {
		return Err(Error::Invalid("SKILL_BINDING_LIMIT".into()));
	}
	if (!config.skill_attachments.is_empty() || !config.skill_roots.is_empty())
		&& !config.core_capabilities.skills
	{
		return Err(Error::Invalid(
			"direct Skills require core_capabilities.skills".into(),
		));
	}
	let mut ids = BTreeSet::new();
	for a in &config.skill_attachments {
		validate(a)?;
		if !ids.insert(a.skill_id) {
			return Err(Error::Invalid("DUPLICATE_SKILL_ID".into()));
		}
	}
	for root in &config.skill_roots {
		crate::registry::rules::validate_path(root)?;
		if root != ".agents/skills" && !root.ends_with("/.agents/skills") {
			return Err(Error::Invalid(
				"Skill discovery requires a mounted .agents/skills root".into(),
			));
		}
	}
	Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pinned {
	#[serde(default)]
	pub loaded: bool,
	pub metadata: SkillMetadata,
	pub files: Vec<super::operations::MountedFile>,
	/// Escaped SKILL.md length when embedded in a JSON system message.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub instruction_json_len: Option<usize>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillList {
	pub cursor: Option<usize>,
	pub limit: Option<usize>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillLoad {
	pub skill_id: Uuid,
	pub expected_digest: String,
}
#[derive(Debug, Serialize, Deserialize)]
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
/// Import assigns a fresh immutable identity and canonicalizes file order before validation.
pub fn imported(
	origin: String,
	instructions: String,
	files: Vec<crate::registry::SkillFile>,
) -> Result<SkillAttachment> {
	let mut attachment = SkillAttachment {
		skill_id: Uuid::new_v4(),
		origin,
		digest: String::new(),
		instructions,
		files,
	};
	attachment.files.sort_by(|a, b| a.path.cmp(&b.path));
	attachment.digest = content_digest(&attachment);
	validate(&attachment)?;
	Ok(attachment)
}
