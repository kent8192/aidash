use crate::apps::workspaces::{serializers::*, services::channels::Channels};
use crate::http::json::Json;
use reinhardt::{Body, Depends, Path, Query, Response, http::ViewResult};
use reinhardt::{get, post};
use uuid::Uuid;

#[post(
	"/api/workspaces/{id}/threads",
	name = "channel-thread-create",
	auth = "protected"
)]
pub async fn channel_thread_create(
	#[inject] channels: Depends<Channels>,
	Path(id): Path<Uuid>,
	Json(input): Json<ChannelThreadInput>,
) -> ViewResult<Response> {
	crate::http::json(channels.create_thread(id, input).await)
}

#[post(
	"/api/workspaces/{id}/thread-messages",
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
	"/api/workspaces/{id}/message-history",
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
	"/api/workspaces/{id}/attachments",
	name = "channel-attachment-upload",
	auth = "protected"
)]
pub async fn channel_attachment_upload(
	#[inject] channels: Depends<Channels>,
	Path(id): Path<Uuid>,
	Query(input): Query<ChannelAttachmentUploadQuery>,
	Body(body): Body,
) -> ViewResult<Response> {
	crate::http::json(channels.upload(id, input, &body).await)
}

#[get(
	"/api/workspaces/{id}/attachments/{attachment_id}",
	name = "channel-attachment-download",
	auth = "protected"
)]
pub async fn channel_attachment_download(
	#[inject] channels: Depends<Channels>,
	Path((workspace, attachment)): Path<(Uuid, Uuid)>,
) -> ViewResult<Response> {
	let result = async {
		let (attachment, content) = channels.download(workspace, attachment).await?;
		Ok(Response::ok()
			.with_body(content)
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
