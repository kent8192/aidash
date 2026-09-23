use super::{
	ChannelAttachment, ChannelAttachmentUploadQuery, ChannelHistoryQuery, ChannelMessage,
	ChannelMessageInput, ChannelMessagePage, ChannelThread, ChannelThreadInput, access::Lease,
	attachments, history, threads,
};
use crate::{Result, authorization::identity::Actor, federation::Federation};
use axum::{
	Extension, Json,
	body::Bytes,
	extract::{Path, Query, State},
	http::{HeaderValue, header},
	response::Response,
};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new()
		.routes(routes!(channel_thread_create))
		.routes(routes!(channel_message_create))
		.routes(routes!(channel_message_history))
		.routes(routes!(channel_attachment_upload))
		.routes(routes!(channel_attachment_download))
}

#[utoipa::path(post, path="/workspaces/{id}/threads", operation_id="channel_thread_create",
	request_body=ChannelThreadInput, params(("id"=Uuid,Path)),
	responses((status=200,body=ChannelThread)), security(("bearer_auth"=[])))]
async fn channel_thread_create(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<ChannelThreadInput>,
) -> Result<Json<ChannelThread>> {
	let mut lease = Lease::begin(&f.store, actor, id, "message.create").await?;
	let result = threads::create(&f.store, &mut lease, id, input.root_message_id).await;
	lease.finish(result).await.map(Json)
}

#[utoipa::path(post, path="/workspaces/{id}/thread-messages", operation_id="channel_message_create",
	request_body=ChannelMessageInput, params(("id"=Uuid,Path)),
	responses((status=200,body=ChannelMessage)), security(("bearer_auth"=[])))]
async fn channel_message_create(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<ChannelMessageInput>,
) -> Result<Json<ChannelMessage>> {
	let mut lease = Lease::begin(&f.store, actor, id, "message.create").await?;
	let result = threads::post(&f.store, &mut lease, id, input).await;
	lease.finish(result).await.map(Json)
}

#[utoipa::path(get, path="/workspaces/{id}/message-history", operation_id="channel_message_history",
	params(("id"=Uuid,Path),ChannelHistoryQuery),
	responses((status=200,body=ChannelMessagePage)), security(("bearer_auth"=[])))]
async fn channel_message_history(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Query(input): Query<ChannelHistoryQuery>,
) -> Result<Json<ChannelMessagePage>> {
	let mut lease = Lease::begin(&f.store, actor, id, "workspace.read").await?;
	let result = history::page(&mut lease, id, input).await;
	lease.finish(result).await.map(Json)
}

#[utoipa::path(post, path="/workspaces/{id}/attachments", operation_id="channel_attachment_upload",
	params(("id"=Uuid,Path),ChannelAttachmentUploadQuery),
	request_body(content=String, content_type="application/octet-stream"),
	responses((status=200,body=ChannelAttachment)), security(("bearer_auth"=[])))]
async fn channel_attachment_upload(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(workspace): Path<Uuid>,
	Query(query): Query<ChannelAttachmentUploadQuery>,
	body: Bytes,
) -> Result<Json<ChannelAttachment>> {
	let mut lease = Lease::begin(&f.store, actor, workspace, "message.create").await?;
	let result = attachments::upload(&mut lease, workspace, query, &body).await;
	lease.finish(result).await.map(Json)
}

#[utoipa::path(get, path="/workspaces/{id}/attachments/{attachment_id}", operation_id="channel_attachment_download",
	params(("id"=Uuid,Path), ("attachment_id"=Uuid,Path)),
	responses((status=200,description="Attachment bytes")), security(("bearer_auth"=[])))]
async fn channel_attachment_download(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((workspace, attachment_id)): Path<(Uuid, Uuid)>,
) -> Result<Response> {
	let mut lease = Lease::begin(&f.store, actor, workspace, "workspace.read").await?;
	let result = attachments::download(&mut lease, workspace, attachment_id).await;
	let (attachment, content) = lease.finish(result).await?;
	let mut response = Response::new(axum::body::Body::from(content));
	let headers = response.headers_mut();
	headers.insert(
		header::CONTENT_TYPE,
		HeaderValue::from_str(&attachment.media_type)
			.unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
	);
	headers.insert(
		header::CONTENT_DISPOSITION,
		HeaderValue::from_str(&format!(
			"attachment; filename*=UTF-8''{}",
			percent_encode(&attachment.filename)
		))
		.map_err(|_| crate::Error::Invalid("invalid attachment filename".into()))?,
	);
	headers.insert(
		"x-content-type-options",
		HeaderValue::from_static("nosniff"),
	);
	Ok(response)
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
