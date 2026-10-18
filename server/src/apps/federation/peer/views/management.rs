//! Management HTTP endpoints.
use crate::http::json::Json;
use crate::{authorization::identity::Actor, federation::Peer, registry::Search};
use reinhardt::Depends;
use reinhardt::Request;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::{get, post};

use crate::apps::federation::peer::services::management::PeerManagement;

#[post("/api/peers", name = "peer-create", auth = "protected")]
pub async fn peer_create(
	#[inject] service: Depends<PeerManagement>,
	Json(peer): Json<Peer>,
) -> ViewResult<Response> {
	crate::http::json(service.peer_create(peer).await)
}

#[post("/api/discover", name = "discover", auth = "protected")]
pub async fn discover(
	#[inject] service: Depends<PeerManagement>,
	#[inject] actor: Actor,
	Json(query): Json<Search>,
) -> ViewResult<Response> {
	crate::http::json(service.discover(actor, query).await)
}

#[get("/api/mesh", name = "mesh", auth = "protected")]
pub async fn mesh(#[inject] service: Depends<PeerManagement>) -> ViewResult<Response> {
	crate::http::json(service.mesh().await)
}

#[get("/.well-known/aidash", name = "identity", auth = "public")]
pub async fn identity(#[inject] service: Depends<PeerManagement>) -> ViewResult<Response> {
	crate::http::json(service.identity().await)
}

#[get("/federation/v0.1/observe", name = "peer-observe", auth = "protected")]
pub async fn peer_observe(
	#[inject] service: Depends<PeerManagement>,
	request: Request,
) -> ViewResult<Response> {
	crate::http::json(service.peer_observe(request.headers.clone()).await)
}
