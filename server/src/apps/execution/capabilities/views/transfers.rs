//! HTTP endpoints backed by injected application services.
use crate::capabilities::transfer as api;
use crate::http::json::Json;
use api::*;
use reinhardt::http::ViewResult;
use reinhardt::{Depends, Request, Response, post};

#[post(
	"/federation/v0.1/scoped/files/describe",
	name = "transfermanagement-describe",
	auth = "protected"
)]
pub async fn describe(
	#[inject] service: Depends<TransferManagement>,
	request: Request,
	Json(input): Json<Identity>,
) -> ViewResult<Response> {
	crate::http::json(api::describe(service.runtime.clone(), request.headers.clone(), input).await)
}
