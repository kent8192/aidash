//! Native peer headers and schema contracts adapt to the receiver application protocol.
use super::{Access, Chunk, Federation, HeaderMap, Identity, Requester, Result, Uuid, Value};
use aidash_application::capabilities::transfer::receiver as application;
pub(crate) async fn negotiate(
	f: Federation,
	headers: HeaderMap,
	input: Requester,
) -> Result<Value> {
	let repository = crate::bootstrap::transfer_repository(&f);
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	application::negotiate(&repository, source, input.into())
		.await
		.map_err(Into::into)
}
pub(crate) async fn prepare(f: Federation, headers: HeaderMap, input: Identity) -> Result<Value> {
	let repository = crate::bootstrap::transfer_repository(&f);
	application::require_admission(&repository).map_err(crate::Error::from)?;
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	application::prepare(&repository, source, input.into())
		.await
		.map_err(Into::into)
}
pub(crate) async fn chunk(f: Federation, headers: HeaderMap, input: Chunk) -> Result<Value> {
	let repository = crate::bootstrap::transfer_repository(&f);
	application::require_admission(&repository).map_err(crate::Error::from)?;
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	application::chunk(&repository, source, input.into())
		.await
		.map_err(Into::into)
}
pub(crate) async fn commit(f: Federation, headers: HeaderMap, input: Identity) -> Result<Value> {
	let repository = crate::bootstrap::transfer_repository(&f);
	application::require_admission(&repository).map_err(crate::Error::from)?;
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	application::commit(&repository, source, input.into())
		.await
		.map_err(Into::into)
}
pub(crate) async fn status(f: Federation, headers: HeaderMap, input: Identity) -> Result<Value> {
	let repository = crate::bootstrap::transfer_repository(&f);
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	application::status(&repository, source, input.into())
		.await
		.map_err(Into::into)
}
pub(crate) async fn recipients(
	f: Federation,
	headers: HeaderMap,
	input: Requester,
) -> Result<Value> {
	let repository = crate::bootstrap::transfer_repository(&f);
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	application::recipients(&repository, source, input.into())
		.await
		.map_err(Into::into)
}
pub(crate) async fn recipient_list(
	f: &Federation,
	access: &mut Access,
	cursor: Option<Uuid>,
) -> Result<Value> {
	application::recipient_list(
		&mut crate::bootstrap::transfer_scope(Some(&f.store), access),
		&f.config.node_id,
		cursor,
	)
	.await
	.map_err(Into::into)
}
#[derive(Clone)]
pub struct TransferReceivers {
	pub(crate) runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> TransferReceivers {
	TransferReceivers { runtime }
}
#[cfg(test)]
use aidash_domain::capabilities::transfer::receiver::{
	MAX_RECIPIENT_VERSIONS_PER_AREA, cap_recipient_versions,
};
use reinhardt::injectable;
#[cfg(test)]
#[path = "../../tests/services_transfer_receiver_recipient_version_tests.rs"]
mod recipient_version_tests;
