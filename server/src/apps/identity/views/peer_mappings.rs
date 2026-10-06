//! Native HTTP endpoints.
use crate::apps::identity::peer::PeerMappings;
use crate::apps::identity::serializers::peer::PeerMappingInput;
use crate::apps::identity::serializers::peer::{DiscoveryInput, HistoryPage, MappingPage};
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Query;
use reinhardt::Request;
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
	Path(tenant): Path<String>,
	Query(page): Query<HistoryPage>,
) -> ViewResult<Response> {
	crate::http::json(service.history(tenant, page).await)
}

#[get(
	"/api/authorization/{tenant}/peer-mappings",
	name = "authorization-peer-mappings",
	auth = "protected"
)]
pub async fn list(
	#[inject] service: Depends<PeerMappings>,
	Path(tenant): Path<String>,
	Query(page): Query<MappingPage>,
) -> ViewResult<Response> {
	crate::http::json(service.list(tenant, page).await)
}

#[post(
	"/api/authorization/{tenant}/peer-mappings",
	name = "authorization-set-peer-mapping",
	auth = "protected"
)]
pub async fn set(
	#[inject] service: Depends<PeerMappings>,
	Path(tenant): Path<String>,
	Json(input): Json<PeerMappingInput>,
) -> ViewResult<Response> {
	crate::http::json(service.set(tenant, input).await)
}

#[post(
	"/federation/v0.1/scoped/discover",
	name = "peer-mappings-discover",
	auth = "protected"
)]
pub async fn discover(
	#[inject] service: Depends<PeerMappings>,
	request: Request,
	Json(input): Json<DiscoveryInput>,
) -> ViewResult<Response> {
	crate::http::json(service.discover(request.headers, input).await)
}
