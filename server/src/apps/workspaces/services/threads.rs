//! Adapt native contracts to the portable channel use cases.
use super::{ChannelMessage, ChannelMessageInput, ChannelThread, access::Lease};
use crate::{Result, store::Store};
use uuid::Uuid;
pub(crate) async fn create(
	store: &Store,
	lease: &mut Lease,
	workspace: Uuid,
	root: Uuid,
) -> Result<ChannelThread> {
	aidash_application::workspaces::channels::create_thread(
		&mut crate::bootstrap::channel_scope(store, lease),
		workspace,
		root,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn post(
	store: &Store,
	lease: &mut Lease,
	workspace: Uuid,
	input: ChannelMessageInput,
) -> Result<ChannelMessage> {
	aidash_application::workspaces::channels::post(
		&mut crate::bootstrap::channel_scope(store, lease),
		workspace,
		aidash_application::workspaces::channels::MessageSubmission {
			content: &input.content,
			thread_id: input.thread_id,
			idempotency_key: input.idempotency_key,
			attachment_ids: &input.attachment_ids,
		},
	)
	.await
	.map_err(Into::into)
}
