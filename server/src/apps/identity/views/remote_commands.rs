//! HTTP endpoints backed by injected application services.
use crate::apps::identity::services::http_auth::InboundCall;
use crate::authorization::remote::execution::commands as api;
use crate::http::json::Json;
use api::*;
use reinhardt::http::ViewResult;
use reinhardt::{Depends, Response, post};

#[post(
	"/federation/v0.1/scoped/execution/commands",
	name = "remotecommands-handle",
	auth = "protected"
)]
pub async fn handle(
	#[inject] service: Depends<RemoteCommands>,
	#[inject] peer: InboundCall,
	Json(input): Json<Input>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::handle(service.runtime.clone(), peer.0, input).await)
}

#[post(
	"/federation/v0.1/thread-attachment/1/fetch",
	name = "remotecommands-thread-attachment",
	auth = "protected"
)]
pub async fn thread_attachment(
	#[inject] service: Depends<RemoteCommands>,
	#[inject] peer: InboundCall,
	Json(input): Json<AttachmentInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::thread_attachment(service.runtime.clone(), peer.0, input).await)
}

#[post(
	"/federation/v0.1/scoped/execution/deliveries/prepare",
	name = "remotecommands-delivery-prepare",
	auth = "protected"
)]
pub async fn delivery_prepare(
	#[inject] service: Depends<RemoteCommands>,
	#[inject] peer: InboundCall,
	Json(input): Json<DeliveryOpen>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::delivery_prepare(service.runtime.clone(), peer.0, input).await)
}

// The path names its chunks so the chunk body limit applies (`request_body_limit`).
#[post(
	"/federation/v0.1/scoped/execution/deliveries/chunks",
	name = "remotecommands-delivery-chunk",
	auth = "protected"
)]
pub async fn delivery_chunk(
	#[inject] service: Depends<RemoteCommands>,
	#[inject] peer: InboundCall,
	Json(input): Json<DeliveryChunk>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::delivery_chunk(service.runtime.clone(), peer.0, input).await)
}

#[post(
	"/federation/v0.1/scoped/execution/deliveries/commit",
	name = "remotecommands-delivery-commit",
	auth = "protected"
)]
pub async fn delivery_commit(
	#[inject] service: Depends<RemoteCommands>,
	#[inject] peer: InboundCall,
	Json(input): Json<DeliveryOpen>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::delivery_commit(service.runtime.clone(), peer.0, input).await)
}
