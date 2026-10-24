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
