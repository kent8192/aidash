//! HTTP endpoints backed by injected application services.
use crate::capabilities::transfer::receiver as api;
use crate::http::json::Json;
use api::*;
use reinhardt::http::ViewResult;
use reinhardt::{Depends, Request, Response, post};

#[post(
	"/federation/v0.1/scoped/files/negotiate",
	name = "transferreceivers-negotiate",
	auth = "protected"
)]
pub async fn negotiate(
	#[inject] service: Depends<TransferReceivers>,
	request: Request,
	Json(input): Json<Requester>,
) -> ViewResult<Response> {
	crate::http::json(api::negotiate(service.runtime.clone(), request.headers.clone(), input).await)
}

#[post(
	"/federation/v0.1/scoped/files/prepare",
	name = "transferreceivers-prepare",
	auth = "protected"
)]
pub async fn prepare(
	#[inject] service: Depends<TransferReceivers>,
	request: Request,
	Json(input): Json<Identity>,
) -> ViewResult<Response> {
	crate::http::json(api::prepare(service.runtime.clone(), request.headers.clone(), input).await)
}

#[post(
	"/federation/v0.1/scoped/files/chunk",
	name = "transferreceivers-chunk",
	auth = "protected"
)]
pub async fn chunk(
	#[inject] service: Depends<TransferReceivers>,
	request: Request,
	Json(input): Json<Chunk>,
) -> ViewResult<Response> {
	crate::http::json(api::chunk(service.runtime.clone(), request.headers.clone(), input).await)
}

#[post(
	"/federation/v0.1/scoped/files/commit",
	name = "transferreceivers-commit",
	auth = "protected"
)]
pub async fn commit(
	#[inject] service: Depends<TransferReceivers>,
	request: Request,
	Json(input): Json<Identity>,
) -> ViewResult<Response> {
	crate::http::json(api::commit(service.runtime.clone(), request.headers.clone(), input).await)
}

#[post(
	"/federation/v0.1/scoped/files/status",
	name = "transferreceivers-status",
	auth = "protected"
)]
pub async fn status(
	#[inject] service: Depends<TransferReceivers>,
	request: Request,
	Json(input): Json<Identity>,
) -> ViewResult<Response> {
	crate::http::json(api::status(service.runtime.clone(), request.headers.clone(), input).await)
}

#[post(
	"/federation/v0.1/scoped/files/recipients",
	name = "transferreceivers-recipients",
	auth = "protected"
)]
pub async fn recipients(
	#[inject] service: Depends<TransferReceivers>,
	request: Request,
	Json(input): Json<Requester>,
) -> ViewResult<Response> {
	crate::http::json(
		api::recipients(service.runtime.clone(), request.headers.clone(), input).await,
	)
}

use crate::capabilities::transfer::{Chunk, Identity, Requester};
