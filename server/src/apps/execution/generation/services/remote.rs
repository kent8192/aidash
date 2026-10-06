//! Origin-owned provider usage delegates current authority and atomic accounting.
#[path = "remote/dispatch.rs"]
pub(crate) mod dispatch;
#[path = "remote/protocol.rs"]
pub(crate) mod protocol;
use crate::{Result, authorization::access::Access, store::Store};
pub use aidash_domain::generation::remote::Ancestor;
pub(crate) use aidash_domain::generation::remote::{Finalization, Purpose, Reserved, Usage};

pub(crate) async fn lineage(access: &mut Access, node: &str) -> Result<Vec<Ancestor>> {
	aidash_application::generation::reservation::lineage(
		&mut crate::bootstrap::generation_usage_authority_scope(access),
		node,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn reserve(
	access: &mut Access,
	store: &Store,
	usage: &Usage,
) -> Result<Vec<Reserved>> {
	aidash_application::generation::reservation::reserve(
		&mut crate::bootstrap::generation_usage_authority_scope(access),
		&crate::bootstrap::generation_reservation_repository(store),
		usage,
	)
	.await
	.map_err(Into::into)
}
/// Called only by the bound dispatcher's authenticated finalization path.
pub(crate) async fn finalize(store: &Store, usage: &Usage, result: &Finalization) -> Result<()> {
	aidash_application::generation::settlement::finalize(
		&crate::bootstrap::generation_settlement_repository(store),
		usage,
		result,
	)
	.await
	.map_err(Into::into)
}
