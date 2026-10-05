//! Native Skill contracts adapt to shared application workflows.
use super::contracts::{Area, FileEntry};
pub use crate::apps::execution::capabilities::serializers::skills::{
	SkillAttachment, SkillList, SkillLoad, SkillMetadata, SkillRead,
};
use crate::{
	Result, authorization::access::Access, domain::Run, registry::AgentConfig, store::Store,
};
use serde_json::Value;
use uuid::Uuid;
pub(crate) fn imported(value: crate::skill_import::ImportedSkill) -> Result<SkillAttachment> {
	aidash_domain::capabilities::skills::imported(value.source, value.instructions, value.files)
		.map_err(Into::into)
}
pub(crate) async fn pin(
	store: &Store,
	access: &mut Access,
	run: Uuid,
	area: &Area,
	config: &AgentConfig,
) -> Result<()> {
	aidash_application::capabilities::skills::pin(
		&mut crate::bootstrap::file_scope(Some(store), access, None),
		run,
		&area.into(),
		config,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn invoke(
	store: &Store,
	access: &mut Access,
	run: &Run,
	name: &str,
	input: Value,
) -> Result<Value> {
	aidash_application::capabilities::skills::invoke(
		&mut crate::bootstrap::file_scope(Some(store), access, Some(run)),
		&run.metadata(),
		name,
		input,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn context(store: &Store, access: &mut Access, run: &Run) -> Result<String> {
	aidash_application::capabilities::skills::context(
		&mut crate::bootstrap::file_scope(Some(store), access, Some(run)),
		&run.metadata(),
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn mounted(access: &mut Access, run: Uuid) -> Result<Vec<FileEntry>> {
	aidash_application::capabilities::skills::mounted(
		&mut crate::bootstrap::file_scope(None, access, None),
		run,
	)
	.await
	.map(|files| files.into_iter().map(Into::into).collect())
	.map_err(Into::into)
}
#[cfg(test)]
use super::contracts::FileScope;
#[cfg(test)]
use crate::apps::execution::capabilities::serializers::skills::Pinned;
#[cfg(test)]
use serde_json::json;
#[cfg(test)]
fn escaped_instruction_len(text: &str) -> Result<usize> {
	aidash_application::capabilities::skills::escaped_instruction_len(text).map_err(Into::into)
}
#[cfg(test)]
fn pinned_context_reserve(pinned: &[Pinned]) -> Result<usize> {
	aidash_application::capabilities::skills::pinned_context_reserve(
		&pinned.iter().cloned().map(Into::into).collect::<Vec<_>>(),
	)
	.map_err(Into::into)
}
#[cfg(test)]
#[path = "../tests/services_skills_review_tests.rs"]
mod review_tests;
