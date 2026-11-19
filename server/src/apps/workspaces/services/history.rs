//! HTTP pagination inputs adapt to the authorization-aware use case.
use super::{ChannelHistoryQuery, ChannelMessagePage, access::Lease};
use crate::{Result, store::Store};
use uuid::Uuid;
pub(crate) async fn page(
	store: &Store,
	lease: &mut Lease,
	workspace: Uuid,
	input: ChannelHistoryQuery,
) -> Result<ChannelMessagePage> {
	aidash_application::workspaces::channels::history(
		&mut crate::bootstrap::channel_scope(store, lease),
		workspace,
		aidash_application::workspaces::channels::HistoryRequest {
			before: input.before,
			limit: input.limit,
		},
	)
	.await
	.map_err(Into::into)
}
