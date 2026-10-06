//! Registry adapters retain the original module API and call portable use cases.
use crate::Result;
pub use crate::apps::registry::repositories::Registry;
pub(crate) use crate::apps::registry::repositories::register_in;
pub use crate::apps::registry::serializers::contracts::*;
pub use aidash_domain::registry::rules::{digest, valid_skill_file_path};
use serde_json::Value;
pub type Localized = aidash_domain::registry::Localized;
pub fn skill_files(entry: &Entry) -> Result<Vec<SkillFile>> {
	aidash_domain::registry::rules::skill_files(entry).map_err(Into::into)
}
pub fn skill_instructions(entry: &Entry) -> Result<String> {
	aidash_domain::registry::rules::skill_instructions(entry).map_err(Into::into)
}
pub(crate) fn overlay_config(target: &mut Value, overrides: &Value) -> Result<()> {
	aidash_domain::registry::rules::overlay_config(target, overrides).map_err(Into::into)
}
pub fn validate(entry: &Entry) -> Result<()> {
	crate::bootstrap::registry_validation()
		.validate_in(entry, true)
		.map_err(Into::into)
}

pub trait CompactorValidation {
	fn validate(&self) -> Result<()>;
}
impl CompactorValidation for CompactorConfig {
	fn validate(&self) -> Result<()> {
		crate::bootstrap::registry_validation()
			.validate_compactor(self, true)
			.map_err(Into::into)
	}
}
#[path = "knowledge.rs"]
pub mod knowledge;
#[path = "skill_import.rs"]
pub mod skill_import;
