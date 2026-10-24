//! HTTP endpoints backed by native dependency injection.
use crate::apps::federation::transactions::Manifest;
use crate::apps::federation::transactions::serializers::authority::Preflight;
use crate::apps::federation::transactions::services::protocol::Transactions;
use crate::apps::identity::services::http_auth::InboundCall;
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Response;
use reinhardt::StatusCode;
use reinhardt::http::ViewResult;
use reinhardt::{get, post};
use uuid::Uuid;

#[post("/api/transactions", name = "transaction-submit", auth = "protected")]
pub async fn submit(
	#[inject] service: Depends<Transactions>,
	#[inject] actor: Actor,
	Json(manifest): Json<Manifest>,
) -> ViewResult<Response> {
	crate::http::json_status(service.submit(actor, manifest).await, StatusCode::ACCEPTED)
}

#[get("/api/transactions", name = "transactions", auth = "protected")]
pub async fn list(
	#[inject] service: Depends<Transactions>,
	#[inject] actor: Actor,
) -> ViewResult<Response> {
	crate::http::json(service.list(actor).await)
}

#[get(
	"/api/transactions/{id}",
	name = "transaction-details",
	auth = "protected"
)]
pub async fn details(
	#[inject] service: Depends<Transactions>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.details(actor, id).await)
}

#[post(
	"/api/transactions/{id}/abort",
	name = "transaction-abort",
	auth = "protected"
)]
pub async fn abort(
	#[inject] service: Depends<Transactions>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.abort(actor, id).await)
}

#[get(
	"/api/transactions/participants",
	name = "transaction-participants",
	auth = "protected"
)]
pub async fn participants(#[inject] service: Depends<Transactions>) -> ViewResult<Response> {
	crate::http::json(service.participants().await)
}

#[post(
	"/federation/v0.1/transactions/reserve",
	name = "peer-transaction-reserve",
	auth = "protected"
)]
pub async fn reserve(
	#[inject] service: Depends<Transactions>,
	#[inject] peer: InboundCall,
	Json(manifest): Json<Manifest>,
) -> ViewResult<Response> {
	crate::http::json(service.reserve(peer.0, manifest).await)
}

#[post(
	"/federation/v0.1/transactions/prepare",
	name = "peer-transaction-prepare",
	auth = "protected"
)]
pub async fn prepare(
	#[inject] service: Depends<Transactions>,
	#[inject] peer: InboundCall,
	Json(manifest): Json<Manifest>,
) -> ViewResult<Response> {
	crate::http::json(service.prepare(peer.0, manifest).await)
}

#[post(
	"/federation/v0.1/transactions/finish",
	name = "peer-transaction-finish",
	auth = "protected"
)]
pub async fn finish(
	#[inject] service: Depends<Transactions>,
	#[inject] peer: InboundCall,
	Json(manifest): Json<Manifest>,
) -> ViewResult<Response> {
	crate::http::json(service.finish(peer.0, manifest).await)
}

#[get(
	"/federation/v0.1/transactions/{id}/decision",
	name = "peer-transaction-decision",
	auth = "protected"
)]
pub async fn decision(
	#[inject] service: Depends<Transactions>,
	#[inject] peer: InboundCall,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.decision(peer.0, id).await)
}

#[post(
	"/federation/v0.1/transactions/preflight",
	name = "peer-transaction-preflight",
	auth = "protected"
)]
pub async fn preflight(
	#[inject] service: Depends<Transactions>,
	#[inject] peer: InboundCall,
	Json(input): Json<Preflight>,
) -> ViewResult<Response> {
	crate::http::json(service.preflight(peer.0, input).await)
}
#[post(
	"/federation/v0.1/transactions/access",
	name = "peer-transaction-access",
	auth = "protected"
)]
pub async fn read_access(
	#[inject] service: Depends<Transactions>,
	#[inject] peer: InboundCall,
	Json(input): Json<Preflight>,
) -> ViewResult<Response> {
	crate::http::json(service.read_access(peer.0, input).await)
}
#[get(
	"/federation/v0.1/transactions/{id}/authority",
	name = "peer-transaction-authority",
	auth = "protected"
)]
pub async fn authority_ticket(
	#[inject] service: Depends<Transactions>,
	#[inject] peer: InboundCall,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.authority_ticket(peer.0, id).await)
}
