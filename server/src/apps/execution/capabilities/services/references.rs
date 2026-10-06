//! Native reference DTO conversion delegates every workflow to application scopes.
use super::{contracts::Area, records::Record};
pub use crate::apps::execution::capabilities::serializers::references::{
	Attachment, Chunk, Reference, Upload,
};
use crate::{Result, authorization::access::Access, registry::AgentConfig, store::Store};
use uuid::Uuid;
pub(crate) async fn get(access: &mut Access, id: Uuid, action: &str) -> Result<Record> {
	let mut scope = crate::bootstrap::reference_scope(None, access, None);
	aidash_application::capabilities::references::get(&mut scope, id, action)
		.await
		.map(|record| crate::apps::execution::repositories::capability_records::native(&record))
		.map_err(Into::into)
}
pub(crate) async fn inspect(access: &mut Access, id: Uuid) -> Result<Reference> {
	let mut scope = crate::bootstrap::reference_scope(None, access, None);
	aidash_application::capabilities::references::inspect(&mut scope, id)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub(crate) async fn start(store: &Store, access: &mut Access, input: Upload) -> Result<Reference> {
	let mut scope = crate::bootstrap::reference_scope(Some(store), access, None);
	aidash_application::capabilities::references::start(&mut scope, input.into())
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub(crate) async fn chunk(
	store: &Store,
	access: &mut Access,
	id: Uuid,
	input: Chunk,
) -> Result<Reference> {
	let mut scope = crate::bootstrap::reference_scope(Some(store), access, None);
	aidash_application::capabilities::references::chunk(&mut scope, id, input.into())
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub(crate) async fn commit(store: &Store, access: &mut Access, id: Uuid) -> Result<Reference> {
	let mut scope = crate::bootstrap::reference_scope(Some(store), access, None);
	aidash_application::capabilities::references::commit(&mut scope, id)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub(crate) async fn revoke(access: &mut Access, id: Uuid, revision: i64) -> Result<()> {
	let mut scope = crate::bootstrap::reference_scope(None, access, None);
	aidash_application::capabilities::references::revoke(&mut scope, id, revision)
		.await
		.map_err(Into::into)
}
pub(crate) async fn pin(
	store: &Store,
	access: &mut Access,
	area: &mut Area,
	config: &AgentConfig,
) -> Result<()> {
	let mut scope = crate::bootstrap::reference_scope(Some(store), access, Some(area));
	aidash_application::capabilities::references::pin(&mut scope, config)
		.await
		.map_err(Into::into)
}
pub(crate) async fn run(store: Store, stopping: tokio::sync::watch::Receiver<bool>) -> Result<()> {
	aidash_runtime::references::run(&crate::bootstrap::reference_repository(&store), stopping)
		.await
		.map_err(Into::into)
}
#[cfg(test)]
use aidash_domain::capabilities::references::lifecycle::next_receipt_cursor;
#[cfg(test)]
#[path = "../tests/services_references_tests.rs"]
mod tests;
