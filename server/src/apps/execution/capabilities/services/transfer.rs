//! Native transfer contracts and peer headers adapt to shared protocol use cases.
#[path = "transfer/receiver.rs"]
pub(crate) mod receiver;
use super::{contracts::Area, sharing::Share};
pub use crate::apps::execution::capabilities::serializers::transfer::{
	Chunk, Description, Identity, Requester,
};
use crate::{
	Result, authorization::access::Access, domain::Run, federation::Federation, store::Store,
};
use http::HeaderMap;
pub(crate) use receiver::recipient_list;
use serde_json::Value;
use uuid::Uuid;
pub(crate) async fn view(access: &mut Access, id: Uuid) -> Result<Value> {
	aidash_application::capabilities::transfer::view(
		&mut crate::bootstrap::transfer_scope(None, access),
		id,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn prepare(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &Area,
	input: Share,
	digest: &str,
) -> Result<Value> {
	aidash_application::capabilities::transfer::prepare(
		&mut crate::bootstrap::transfer_scope(Some(store), access),
		&run.metadata(),
		&area.into(),
		input.into(),
		digest,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn describe(
	f: Federation,
	headers: HeaderMap,
	input: Identity,
) -> Result<Description> {
	let node = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	aidash_application::capabilities::transfer::describe(
		&crate::bootstrap::transfer_repository(&f),
		node,
		input.into(),
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}
pub(crate) async fn reconcile(f: &Federation, id: Uuid) -> Result<()> {
	aidash_application::capabilities::transfer::reconcile(
		&crate::bootstrap::transfer_repository(f),
		id,
	)
	.await
	.map_err(Into::into)
}
pub async fn run(f: Federation, stopping: tokio::sync::watch::Receiver<bool>) -> Result<()> {
	aidash_runtime::transfer::run(&crate::bootstrap::transfer_repository(&f), stopping)
		.await
		.map_err(Into::into)
}
#[derive(Clone)]
pub struct TransferManagement {
	pub(crate) runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> TransferManagement {
	tracing::trace!(
		service = "TransferManagement",
		"creating injectable service"
	);
	TransferManagement { runtime }
}
use reinhardt::injectable;
