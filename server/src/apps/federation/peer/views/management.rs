//! Management HTTP endpoints.
use crate::http::json::Json;
use crate::{
	apps::workspaces::serializers::tasks::PageQuery,
	authorization::identity::Actor,
	federation::{Offer, Peer},
	registry::Search,
};
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Query;
use reinhardt::Request;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::{get, post};

use crate::apps::federation::peer::serializers::management::RemoteControl;
use crate::apps::federation::peer::serializers::management::WorkspaceCommand;
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

#[post(
	"/federation/v0.1/discover",
	name = "peer-discover",
	auth = "protected"
)]
pub async fn peer_discover(
	#[inject] service: Depends<PeerManagement>,
	Query(page): Query<PageQuery>,
	Json(query): Json<Search>,
) -> ViewResult<Response> {
	crate::http::json(service.peer_discover(page, query).await)
}

#[get(
	"/federation/v0.1/discover/{id}/{version}",
	name = "peer-agent",
	auth = "protected"
)]
pub async fn peer_agent(
	#[inject] service: Depends<PeerManagement>,
	Path((id, version)): Path<(String, String)>,
) -> ViewResult<Response> {
	crate::http::json(service.peer_agent((id, version)).await)
}

#[get(
	"/federation/v0.1/discover/{id}/{version}/bindings",
	name = "peer-agent-bindings",
	auth = "protected"
)]
pub async fn peer_agent_bindings(
	#[inject] service: Depends<PeerManagement>,
	Path((id, version)): Path<(String, String)>,
) -> ViewResult<Response> {
	crate::http::json(service.peer_agent_bindings((id, version)).await)
}

#[post("/federation/v0.1/offers", name = "peer-offer", auth = "protected")]
pub async fn peer_offer(
	#[inject] service: Depends<PeerManagement>,
	request: Request,
	Json(offer): Json<Offer>,
) -> ViewResult<Response> {
	crate::http::json(service.peer_offer(request.headers.clone(), offer).await)
}

#[post(
	"/federation/v0.1/workspace",
	name = "peer-workspace",
	auth = "protected"
)]
pub async fn peer_workspace(
	#[inject] service: Depends<PeerManagement>,
	request: Request,
	Json(command): Json<WorkspaceCommand>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.peer_workspace(request.headers.clone(), command)
			.await,
	)
}

#[get("/federation/v0.1/observe", name = "peer-observe", auth = "protected")]
pub async fn peer_observe(
	#[inject] service: Depends<PeerManagement>,
	request: Request,
) -> ViewResult<Response> {
	crate::http::json(service.peer_observe(request.headers.clone()).await)
}

#[post("/federation/v0.1/control", name = "peer-control", auth = "protected")]
pub async fn peer_control(
	#[inject] service: Depends<PeerManagement>,
	request: Request,
	Json(input): Json<RemoteControl>,
) -> ViewResult<Response> {
	crate::http::json(service.peer_control(request.headers.clone(), input).await)
}
