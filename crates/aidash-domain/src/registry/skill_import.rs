//! Skill import requests and snapshots retain the existing HTTP contract.
use serde::{Deserialize, Serialize};

use super::SkillFile;
use schemars::JsonSchema;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
	pub url: String,
	pub skill_path: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ImportResult {
	pub skills: Vec<String>,
	pub selected: Option<ImportedSkill>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ImportedSkill {
	pub path: String,
	pub source: String,
	pub instructions: String,
	pub files: Vec<SkillFile>,
}

/// Imported instructions must remain representable as a Registry Skill.
pub fn validate_imported_instructions(instructions: &str) -> crate::Result<()> {
	if instructions.trim().is_empty() {
		return Err(crate::Error::Invalid("Skill instructions are empty".into()));
	}
	if instructions.len() > 65_536 || instructions.contains('\0') {
		return Err(crate::Error::Invalid(
			"Skill instructions exceed 64 KiB or contain NUL".into(),
		));
	}
	Ok(())
}
