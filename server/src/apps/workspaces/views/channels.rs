use crate::apps::workspaces::{serializers::*, services::channels::Channels};
use crate::http::json::Json;
use reinhardt::{Depends, Path, Query, Request, Response, http::ViewResult};
use reinhardt::{get, post, put};
use uuid::Uuid;

#[post(
	"/api/threads/{id}/channel-messages",
	name = "channel-message-create",
	auth = "protected"
)]
pub async fn channel_message_create(
	#[inject] channels: Depends<Channels>,
	Path(id): Path<Uuid>,
	Json(input): Json<ChannelMessageInput>,
) -> ViewResult<Response> {
	crate::http::json(channels.post_message(id, input).await)
}

#[get(
	"/api/threads/{id}/message-history",
	name = "channel-message-history",
	auth = "protected"
)]
pub async fn channel_message_history(
	#[inject] channels: Depends<Channels>,
	Path(id): Path<Uuid>,
	Query(input): Query<ChannelHistoryQuery>,
) -> ViewResult<Response> {
	crate::http::json(channels.history(id, input).await)
}

#[post(
	"/api/threads/{id}/attachments",
	name = "channel-attachment-upload",
	auth = "protected"
)]
pub async fn channel_attachment_upload(
	#[inject] channels: Depends<Channels>,
	Path(id): Path<Uuid>,
	Query(input): Query<ChannelAttachmentUploadQuery>,
	request: Request,
) -> ViewResult<Response> {
	// Gateway admission has already bounded this route to 8 MiB; the generic
	// Body extractor's fixed 2 MiB limit is too small for attachments.
	crate::http::json(channels.upload(id, input, request.body()).await)
}

#[put(
	"/api/threads/{id}/attachments/{attachment_id}/description",
	name = "channel-attachment-description",
	auth = "protected"
)]
pub async fn channel_attachment_description(
	#[inject] channels: Depends<Channels>,
	Path((thread, attachment)): Path<(Uuid, Uuid)>,
	Json(input): Json<ChannelAttachmentDescription>,
) -> ViewResult<Response> {
	crate::http::json(channels.describe(thread, attachment, input).await)
}

#[get(
	"/api/threads/{id}/attachments/{attachment_id}",
	name = "channel-attachment-download",
	auth = "protected"
)]
pub async fn channel_attachment_download(
	#[inject] channels: Depends<Channels>,
	Path((thread, attachment)): Path<(Uuid, Uuid)>,
) -> ViewResult<Response> {
	let result = async {
		let (attachment, content) = channels.download(thread, attachment).await?;
		use crate::apps::workspaces::services::attachments::Download;
		let response = match content {
			Download::Buffered(bytes) => Response::ok().with_body(bytes),
			Download::Blob(mut reader) => {
				use tokio::io::AsyncReadExt as _;
				let mut remaining = u64::try_from(attachment.size_bytes)
					.map_err(|_| crate::Error::Conflict("OBJECT_INTEGRITY".into()))?;
				let stream: reinhardt::http::StreamBody = Box::pin(async_stream::try_stream! {
				  while remaining > 0 {
					let length = remaining.min(64 * 1024) as usize;
					let mut buffer = vec![0; length];
					reader.read_exact(&mut buffer).await?;
					remaining -= length as u64;
					yield bytes::Bytes::from(buffer);
				  }
				});
				Response::ok()
					.with_stream(stream)
					.with_header("Content-Length", &attachment.size_bytes.to_string())
			}
		};
		Ok(response
			.with_header("Content-Type", &attachment.media_type)
			.with_header(
				"Content-Disposition",
				&format!(
					"attachment; filename*=UTF-8''{}",
					percent_encode(&attachment.filename)
				),
			)
			.with_header("X-Content-Type-Options", "nosniff"))
	}
	.await;
	crate::http::response(result)
}

fn percent_encode(value: &str) -> String {
	let mut encoded = String::new();
	for byte in value.bytes() {
		if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
			encoded.push(char::from(byte));
		} else {
			encoded.push_str(&format!("%{byte:02X}"));
		}
	}
	encoded
}
