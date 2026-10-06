//! HTTP endpoints backed by native dependency injection.
use crate::apps::federation::transactions::Manifest;
use crate::apps::federation::transactions::serializers::authority::Preflight;
use crate::apps::federation::transactions::serializers::protocol::PeerRecovery;
use crate::apps::federation::transactions::serializers::protocol::TransactionTrust;
use crate::apps::federation::transactions::services::protocol::Transactions;
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Request;
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

#[get(
	"/api/transactions/trust",
	name = "transaction-trust-list",
	auth = "protected"
)]
pub async fn trust_list(#[inject] service: Depends<Transactions>) -> ViewResult<Response> {
	crate::http::json(service.trust_list().await)
}

#[post(
	"/api/transactions/trust",
	name = "transaction-trust",
	auth = "protected"
)]
pub async fn trust(
	#[inject] service: Depends<Transactions>,
	Json(input): Json<TransactionTrust>,
) -> ViewResult<Response> {
	match service.trust(input).await {
		Ok((status, value)) => crate::http::json_status(Ok(value), status),
		Err(error) => Ok(error.http_response()),
	}
}

#[post(
	"/federation/v0.1/transactions/reserve",
	name = "peer-transaction-reserve",
	auth = "protected"
)]
pub async fn reserve(
	#[inject] service: Depends<Transactions>,
	request: Request,
	Json(manifest): Json<Manifest>,
) -> ViewResult<Response> {
	crate::http::json(service.reserve(request.headers, manifest).await)
}

#[post(
	"/federation/v0.1/transactions/prepare",
	name = "peer-transaction-prepare",
	auth = "protected"
)]
pub async fn prepare(
	#[inject] service: Depends<Transactions>,
	request: Request,
	Json(manifest): Json<Manifest>,
) -> ViewResult<Response> {
	crate::http::json(service.prepare(request.headers, manifest).await)
}

#[post(
	"/federation/v0.1/transactions/finish",
	name = "peer-transaction-finish",
	auth = "protected"
)]
pub async fn finish(
	#[inject] service: Depends<Transactions>,
	request: Request,
	Json(manifest): Json<Manifest>,
) -> ViewResult<Response> {
	crate::http::json(service.finish(request.headers, manifest).await)
}

#[get(
	"/federation/v0.1/transactions/{id}/decision",
	name = "peer-transaction-decision",
	auth = "protected"
)]
pub async fn decision(
	#[inject] service: Depends<Transactions>,
	request: Request,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.decision(request.headers, id).await)
}

#[post(
	"/api/transactions/peer-recovery",
	name = "transaction-peer-recovery",
	auth = "protected"
)]
pub async fn restore_peer(
	#[inject] service: Depends<Transactions>,
	Json(input): Json<PeerRecovery>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(service.restore_peer(input).await)
}
#[post(
	"/federation/v0.1/transactions/preflight",
	name = "peer-transaction-preflight",
	auth = "protected"
)]
pub async fn preflight(
	#[inject] service: Depends<Transactions>,
	request: Request,
	Json(input): Json<Preflight>,
) -> ViewResult<Response> {
	crate::http::json(service.preflight(request.headers, input).await)
}
#[post(
	"/federation/v0.1/transactions/access",
	name = "peer-transaction-access",
	auth = "protected"
)]
pub async fn read_access(
	#[inject] service: Depends<Transactions>,
	request: Request,
	Json(input): Json<Preflight>,
) -> ViewResult<Response> {
	crate::http::json(service.read_access(request.headers, input).await)
}
#[get(
	"/federation/v0.1/transactions/{id}/authority",
	name = "peer-transaction-authority",
	auth = "protected"
)]
pub async fn authority_ticket(
	#[inject] service: Depends<Transactions>,
	request: Request,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.authority_ticket(request.headers, id).await)
}
