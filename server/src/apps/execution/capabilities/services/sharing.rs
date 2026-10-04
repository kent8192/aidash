//! HTTP/native sharing DTOs adapt to one application snapshot-delivery transaction.
use super::contracts::Area;
pub use crate::apps::execution::capabilities::serializers::sharing::{Recipient, Selection, Share};
pub(crate) use crate::apps::execution::repositories::sharing::serialize;
use crate::{Result, authorization::access::Access, domain::Run, store::Store};
use serde_json::Value;
pub(crate) async fn share(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &Area,
	input: Share,
) -> Result<Value> {
	aidash_application::capabilities::sharing::share(
		&mut crate::bootstrap::file_scope(Some(store), access, Some(run)),
		&run.metadata(),
		&area.into(),
		input.into(),
	)
	.await
	.map_err(Into::into)
}
