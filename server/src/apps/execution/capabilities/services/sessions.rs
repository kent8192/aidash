//! Native entry points adapt session DTOs to shared application authority and workflows.
use super::contracts::{Area, SessionStatus};
pub(crate) use crate::apps::execution::repositories::sessions::select;
use crate::{
	Result,
	authorization::access::Access,
	domain::{Run, Task},
	registry::AgentConfig,
	store::Store,
};
use aidash_application::capabilities::sessions as application;
use serde_json::Value;
use uuid::Uuid;
pub(crate) async fn bind_task(access: &mut Access, task: Uuid, thread: Uuid) -> Result<()> {
	application::bind_task(
		&mut crate::bootstrap::session_scope(None, access),
		task,
		thread,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn load(access: &mut Access, id: Uuid) -> Result<Area> {
	application::load(&mut crate::bootstrap::session_scope(None, access), id)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub(crate) async fn authorize(access: &mut Access, area: &Area, action: &str) -> Result<()> {
	application::authorize(
		&mut crate::bootstrap::session_scope(None, access),
		&area.into(),
		action,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn authorize_sources(
	access: &mut Access,
	workspace: Uuid,
	constraints: &Value,
) -> Result<()> {
	application::authorize_sources(
		&mut crate::bootstrap::session_scope(None, access),
		workspace,
		constraints,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn for_run(access: &mut Access, run: &Run) -> Result<Area> {
	application::for_run(
		&mut crate::bootstrap::session_scope(None, access),
		&run.metadata(),
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}
pub(crate) async fn require_current_run(access: &mut Access, area: &Area, run: &Run) -> Result<()> {
	application::require_current_run(
		&mut crate::bootstrap::session_scope(None, access),
		&area.into(),
		&run.metadata(),
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn context_authority(
	access: &mut Access,
	run: impl Into<crate::domain::RunMetadata>,
) -> Result<()> {
	application::context_authority(
		&mut crate::bootstrap::session_scope(None, access),
		&run.into(),
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn prepare_admission(
	store: &Store,
	access: &mut Access,
	task: &Task,
	config: &AgentConfig,
	agent_id: &str,
) -> Result<Option<Uuid>> {
	application::prepare_admission(
		&mut crate::bootstrap::session_scope(Some(store), access),
		task,
		config,
		agent_id,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn admit(
	store: &Store,
	access: &mut Access,
	task: &Task,
	run_id: Uuid,
	thread: Option<Uuid>,
	config: &AgentConfig,
	agent_id: &str,
) -> Result<()> {
	application::admit(
		&mut crate::bootstrap::session_scope(Some(store), access),
		task,
		run_id,
		thread,
		config,
		agent_id,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn initialize(
	store: &Store,
	access: &mut Access,
	run: &Run,
	config: &AgentConfig,
) -> Result<()> {
	application::initialize(
		&mut crate::bootstrap::session_scope(Some(store), access),
		&run.metadata(),
		config,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn status(access: &mut Access, area: &Area) -> Result<SessionStatus> {
	application::status(
		&mut crate::bootstrap::session_scope(None, access),
		&area.into(),
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}
pub(crate) async fn cached(access: &mut Access, key: Uuid, digest: &str) -> Result<Option<Value>> {
	application::cached(
		&mut crate::bootstrap::session_scope(None, access),
		key,
		digest,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn cache(
	access: &mut Access,
	key: Uuid,
	digest: &str,
	result: &Value,
) -> Result<()> {
	application::cache(
		&mut crate::bootstrap::session_scope(None, access),
		key,
		digest,
		result,
	)
	.await
	.map_err(Into::into)
}
#[cfg(test)]
fn received_scope_matches(subjects: &[String], owner: &str, agent: &str) -> bool {
	aidash_domain::capabilities::sessions::received_scope_matches(subjects, owner, agent)
}
#[cfg(test)]
#[path = "../tests/services_sessions_tests.rs"]
mod tests;
