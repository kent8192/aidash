//! Native peer headers and schema contracts adapt to the receiver application protocol.
use super::{Access, Chunk, Federation, Identity, Requester, Result, Uuid, Value};
use aidash_application::capabilities::transfer::receiver as application;
use aidash_application::federation::inbound::Admitted;
pub(crate) async fn negotiate(
	f: Federation,
	admitted: Admitted,
	input: Requester,
) -> Result<Value> {
	let repository =
		crate::bootstrap::transfer_receiver_repository(&f, admitted.receiver.node.clone());
	let source = &admitted.sender.to_string();
	application::negotiate(&repository, source, input.into())
		.await
		.map_err(Into::into)
}
pub(crate) async fn prepare(f: Federation, admitted: Admitted, input: Identity) -> Result<Value> {
	let repository =
		crate::bootstrap::transfer_receiver_repository(&f, admitted.receiver.node.clone());
	application::require_admission(&repository).map_err(crate::Error::from)?;
	let source = &admitted.sender.to_string();
	application::prepare(&repository, source, input.into())
		.await
		.map_err(Into::into)
}
pub(crate) async fn chunk(f: Federation, admitted: Admitted, input: Chunk) -> Result<Value> {
	let repository =
		crate::bootstrap::transfer_receiver_repository(&f, admitted.receiver.node.clone());
	application::require_admission(&repository).map_err(crate::Error::from)?;
	let source = &admitted.sender.to_string();
	application::chunk(&repository, source, input.into())
		.await
		.map_err(Into::into)
}
pub(crate) async fn commit(f: Federation, admitted: Admitted, input: Identity) -> Result<Value> {
	let repository =
		crate::bootstrap::transfer_receiver_repository(&f, admitted.receiver.node.clone());
	application::require_admission(&repository).map_err(crate::Error::from)?;
	let source = &admitted.sender.to_string();
	application::commit(&repository, source, input.into())
		.await
		.map_err(Into::into)
}
pub(crate) async fn status(f: Federation, admitted: Admitted, input: Identity) -> Result<Value> {
	let repository =
		crate::bootstrap::transfer_receiver_repository(&f, admitted.receiver.node.clone());
	let source = &admitted.sender.to_string();
	application::status(&repository, source, input.into())
		.await
		.map_err(Into::into)
}
pub(crate) async fn recipients(
	f: Federation,
	admitted: Admitted,
	input: Requester,
) -> Result<Value> {
	let repository =
		crate::bootstrap::transfer_receiver_repository(&f, admitted.receiver.node.clone());
	let source = &admitted.sender.to_string();
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
		f.config.deployment.as_str(),
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
