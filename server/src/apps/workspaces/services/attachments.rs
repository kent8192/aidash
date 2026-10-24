//! Attachment transport inputs adapt to the channel use cases.
use super::{ChannelAttachment, ChannelAttachmentUploadQuery, access::Lease};
use crate::{Result, store::Store};
use uuid::Uuid;
pub(crate) async fn upload(
	store: &Store,
	lease: &mut Lease,
	workspace: Uuid,
	input: ChannelAttachmentUploadQuery,
	content: &[u8],
) -> Result<ChannelAttachment> {
	aidash_application::workspaces::channels::upload(
		&mut crate::bootstrap::channel_scope(store, lease),
		workspace,
		aidash_application::workspaces::channels::AttachmentUpload {
			filename: &input.filename,
			media_type: &input.media_type,
			idempotency_key: input.idempotency_key,
		},
		content,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn download(
	store: &Store,
	lease: &mut Lease,
	workspace: Uuid,
	id: Uuid,
) -> Result<(ChannelAttachment, Vec<u8>)> {
	aidash_application::workspaces::channels::download(
		&mut crate::bootstrap::channel_scope(store, lease),
		workspace,
		id,
	)
	.await
	.map_err(Into::into)
}
