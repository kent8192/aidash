//! Native lifecycle entry points adapt unchanged contracts to application workflows.
use super::{contracts::Area, records::Record};
pub use crate::apps::execution::capabilities::serializers::cleanup::{
	Choice, Cleanup, CleanupResult, ManagedArea, ManagementPage, Restore, RestoreNewThread,
};
pub(crate) use crate::apps::execution::repositories::cleanup::persist;
use crate::{Result, authorization::access::Access, store::Store};
use aidash_application::capabilities::cleanup as application;
use serde_json::Value;
use uuid::Uuid;
pub(crate) async fn load(access: &mut Access, id: Uuid, action: &str) -> Result<Area> {
	application::load(
		&mut crate::bootstrap::cleanup_scope(None, access),
		id,
		action,
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}
pub(crate) async fn inventory(access: &mut Access, cursor: Option<Uuid>) -> Result<ManagementPage> {
	application::inventory(&mut crate::bootstrap::cleanup_scope(None, access), cursor)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub(crate) async fn confirmation(access: &mut Access, id: Uuid, revision: i64) -> Result<Value> {
	application::confirmation(
		&mut crate::bootstrap::cleanup_scope(None, access),
		id,
		revision,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn prepare(
	store: &Store,
	access: &mut Access,
	id: Uuid,
	input: Cleanup,
) -> Result<CleanupResult> {
	application::prepare(
		&mut crate::bootstrap::cleanup_scope(Some(store), access),
		id,
		input.into(),
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}
pub(crate) async fn status(access: &mut Access, id: Uuid) -> Result<CleanupResult> {
	application::status(&mut crate::bootstrap::cleanup_scope(None, access), id)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub(crate) async fn restore(
	store: &Store,
	access: &mut Access,
	id: Uuid,
	input: Restore,
) -> Result<Area> {
	application::restore(
		&mut crate::bootstrap::cleanup_scope(Some(store), access),
		id,
		input.into(),
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}
pub(crate) async fn erase_job(store: &Store, snapshot: Record) -> Result<()> {
	application::erase_job(
		&crate::bootstrap::cleanup_repository(store),
		crate::apps::execution::repositories::capability_records::domain(snapshot),
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn run(store: Store, stopping: tokio::sync::watch::Receiver<bool>) -> Result<()> {
	aidash_runtime::cleanup::run(&crate::bootstrap::cleanup_repository(&store), stopping)
		.await
		.map_err(Into::into)
}
