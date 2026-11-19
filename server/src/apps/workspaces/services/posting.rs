//! Adapt native contracts to the portable channel use cases.
use super::{ChannelMessage, ChannelMessageInput, access::Lease};
use crate::{Result, store::Store};
use uuid::Uuid;
pub(crate) async fn post<'a>(
	store: &'a Store,
	federation: Option<&'a crate::federation::Federation>,
	lease: &'a mut Lease,
	workspace: Uuid,
	input: ChannelMessageInput,
) -> Result<ChannelMessage> {
	let mut scope = crate::bootstrap::channel_scope(store, lease);
	scope.federation = federation;
	aidash_application::workspaces::channels::post(
		&mut scope,
		workspace,
		aidash_application::workspaces::channels::MessageSubmission {
			content: &input.content,
			idempotency_key: input.idempotency_key,
			attachment_ids: &input.attachment_ids,
			citations: &input.file_versions,
		},
	)
	.await
	.map_err(Into::into)
}
