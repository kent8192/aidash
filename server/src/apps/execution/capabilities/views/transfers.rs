//! HTTP endpoints backed by injected application services.
use crate::apps::identity::services::http_auth::InboundCall;
use crate::capabilities::transfer as api;
use crate::http::json::Json;
use api::*;
use reinhardt::http::ViewResult;
use reinhardt::{Depends, Response, post};

#[post(
	"/federation/v0.1/scoped/files/describe",
	name = "transfermanagement-describe",
	auth = "protected"
)]
pub async fn describe(
	#[inject] service: Depends<TransferManagement>,
	#[inject] peer: InboundCall,
	Json(input): Json<Identity>,
) -> ViewResult<Response> {
	crate::http::json(api::describe(service.runtime.clone(), peer.0, input).await)
}
