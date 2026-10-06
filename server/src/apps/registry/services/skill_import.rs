//! Skill import HTTP adapter uses the source assembled in bootstrap.
use crate::Result;
pub use crate::apps::registry::serializers::skill_import::{
	ImportRequest, ImportResult, ImportedSkill,
};
pub async fn import(request: ImportRequest) -> Result<ImportResult> {
	crate::bootstrap::skill_import()
		.import(request)
		.await
		.map_err(Into::into)
}
