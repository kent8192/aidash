//! Fetch a portable Skill snapshot through the injected external source.
use crate::{Result, ports::registry::SkillSource};
use aidash_domain::registry::skill_import::{ImportRequest, ImportResult};
use std::sync::Arc;

#[derive(Clone)]
pub struct SkillImport {
	source: Arc<dyn SkillSource>,
}
impl SkillImport {
	pub fn new(source: Arc<dyn SkillSource>) -> Self {
		Self { source }
	}
	pub async fn import(&self, request: ImportRequest) -> Result<ImportResult> {
		self.source.import(request).await
	}
}
