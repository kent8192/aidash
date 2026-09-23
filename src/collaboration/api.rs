use super::{
	ChannelHistoryQuery, ChannelMessage, ChannelMessageInput, ChannelMessagePage, ChannelThread,
	ChannelThreadInput, access::Lease, history, threads,
};
use crate::{Result, authorization::identity::Actor, federation::Federation};
use axum::{
	Extension, Json,
	extract::{Path, Query, State},
};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new()
		.routes(routes!(channel_thread_create))
		.routes(routes!(channel_message_create))
		.routes(routes!(channel_message_history))
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
