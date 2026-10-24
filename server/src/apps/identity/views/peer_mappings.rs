//! Native HTTP endpoints.
use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::peer::PeerMappings;
use crate::apps::identity::serializers::peer::PeerMappingInput;
use crate::apps::identity::serializers::peer::{DiscoveryInput, HistoryPage, MappingPage};
use crate::apps::identity::services::http_auth::InboundCall;
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Query;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::{get, post};

#[get(
	"/api/authorization/{tenant}/peer-mapping-history",
	name = "authorization-peer-mapping-history",
	auth = "protected"
)]
pub async fn history(
	#[inject] service: Depends<PeerMappings>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Query(page): Query<HistoryPage>,
) -> ViewResult<Response> {
	crate::http::json(service.history(actor, origin, tenant, page).await)
}

#[get(
	"/api/authorization/{tenant}/peer-mappings",
	name = "authorization-peer-mappings",
	auth = "protected"
)]
pub async fn list(
	#[inject] service: Depends<PeerMappings>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Query(page): Query<MappingPage>,
) -> ViewResult<Response> {
	crate::http::json(service.list(actor, origin, tenant, page).await)
}

#[post(
	"/api/authorization/{tenant}/peer-mappings",
	name = "authorization-set-peer-mapping",
	auth = "protected"
)]
pub async fn set(
	#[inject] service: Depends<PeerMappings>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(input): Json<PeerMappingInput>,
) -> ViewResult<Response> {
	crate::http::json(service.set(actor, origin, tenant, input).await)
}

#[post(
	"/federation/v0.1/scoped/discover",
	name = "peer-mappings-discover",
	auth = "protected"
)]
pub async fn discover(
	#[inject] service: Depends<PeerMappings>,
	#[inject] peer: InboundCall,
	Json(input): Json<DiscoveryInput>,
) -> ViewResult<Response> {
	crate::http::json(service.discover(peer.0, input).await)
}
