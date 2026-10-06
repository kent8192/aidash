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
		crate::tool::builtin_contract(name)
			.and_then(|contract| contract.authorization.core)
			.is_some_and(|permission| permission.permitted(self))
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

pub mod errors;
pub mod files;

pub mod configuration;
pub mod patch;

pub mod thread_lifecycle;

pub mod sharing;

pub mod transfer;
