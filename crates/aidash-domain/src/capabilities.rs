//! Declared agent capabilities and immutable knowledge attachments.
use crate::registry::SkillFile;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct CoreCapabilities {
	pub files: bool,
	pub shell: bool,
	pub python: bool,
	pub patch: bool,
	pub skills: bool,
	pub sharing: bool,
}

impl CoreCapabilities {
	pub fn enabled(&self) -> bool {
		self.files || self.shell || self.python || self.patch || self.skills || self.sharing
	}

	pub fn permits(&self, name: &str) -> bool {
		match name {
			"file_search" | "file_read" => self.files,
			"shell" | "shell_poll" | "shell_cancel" => self.shell,
			"code_interpreter" | "python_install" | "python_poll" | "python_cancel" => self.python,
			"apply_patch" => self.patch,
			"skill_list" | "skill_load" | "skill_read" => self.skills,
			"file_share" => self.sharing,
			"outbound_get" => self.shell || self.python,
			_ => false,
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillAttachment {
	pub skill_id: Uuid,
	pub origin: String,
	pub digest: String,
	pub instructions: String,
	pub files: Vec<SkillFile>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "Attachment")]
pub struct ReferenceAttachment {
	pub reference_id: Uuid,
	pub digest: String,
}

pub mod references;
pub mod skills;

pub mod operations;

pub mod outbound;
pub mod records;

pub mod approvals;

pub mod reclamation;

pub mod sessions;

pub mod cleanup;

pub mod packages;
pub mod python;
