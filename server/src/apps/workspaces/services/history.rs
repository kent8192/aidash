//! HTTP pagination inputs adapt to the authorization-aware use case.
use super::{ChannelHistoryQuery, ChannelMessagePage, access::Lease};
use crate::{Result, store::Store};
use uuid::Uuid;
pub(crate) async fn page(
	store: &Store,
	lease: &mut Lease,
	thread: Uuid,
	input: ChannelHistoryQuery,
) -> Result<ChannelMessagePage> {
	aidash_application::threads::channels::history(
		&mut crate::bootstrap::channel_scope(store, lease),
		thread,
		aidash_application::threads::channels::HistoryRequest {
			before: input.before,
			limit: input.limit,
		},
	)
	.await
	.map_err(Into::into)
}
