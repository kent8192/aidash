//! HTTP endpoints backed by injected application services.
use crate::apps::identity::services::http_auth::InboundCall;
use crate::authorization::peer::admission as api;
use crate::http::json::Json;
use api::*;
use reinhardt::http::ViewResult;
use reinhardt::{Depends, Path, Response, post};
use uuid::Uuid;

#[post(
	"/federation/v0.1/scoped/execution/admissions/{id}/activate",
	name = "peerexecutionmanagement-activate",
	auth = "protected"
)]
pub async fn activate(
	#[inject] service: Depends<PeerExecutionManagement>,
	#[inject] peer: InboundCall,
	Path(id): Path<Uuid>,
	Json(input): Json<Input>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::activate(service.runtime.clone(), peer.0, id, input).await)
}

#[post(
	"/federation/v0.1/scoped/execution/admissions/{id}/control",
	name = "peerexecutionmanagement-control",
	auth = "protected"
)]
pub async fn control(
	#[inject] service: Depends<PeerExecutionManagement>,
	#[inject] peer: InboundCall,
	Path(id): Path<Uuid>,
	Json(input): Json<RemoteExecutionControlInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::control(service.runtime.clone(), peer.0, id, input).await)
}

#[post(
	"/federation/v0.1/scoped/execution/admissions/{id}/messages",
	name = "peerexecutionmanagement-message",
	auth = "protected"
)]
pub async fn message(
	#[inject] service: Depends<PeerExecutionManagement>,
	#[inject] peer: InboundCall,
	Path(id): Path<Uuid>,
	Json(input): Json<MessageInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::message(service.runtime.clone(), peer.0, id, input).await)
}

#[post(
	"/federation/v0.1/scoped/execution/status",
	name = "peerexecutionmanagement-status",
	auth = "protected"
)]
pub async fn status(
	#[inject] service: Depends<PeerExecutionManagement>,
	#[inject] peer: InboundCall,
	Json(input): Json<Input>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::status(service.runtime.clone(), peer.0, input).await)
}
