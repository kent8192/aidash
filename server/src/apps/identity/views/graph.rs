//! HTTP endpoints backed by injected application services.
use crate::authorization::peer::graph as api;
use crate::http::json::Json;
use api::*;
use reinhardt::http::ViewResult;
use reinhardt::{Depends, Path, Query, Request, Response, get, post};

#[get(
	"/api/federation/graph/peers",
	name = "!federated_graph_peers",
	auth = "protected"
)]
pub async fn peers(
	#[inject] service: Depends<GraphManagement>,
	#[inject] actor: Actor,
	#[inject] origin: Option<crate::dashboard_auth::BrowserOrigin>,
) -> ViewResult<Response> {
	crate::http::json(api::peers(service.runtime.clone(), actor, origin).await)
}

#[post(
	"/api/federation/graph",
	name = "!federated_graph_expand",
	auth = "protected"
)]
pub async fn expand(
	#[inject] service: Depends<GraphManagement>,
	#[inject] actor: Actor,
	#[inject] origin: Option<crate::dashboard_auth::BrowserOrigin>,
	Json(input): Json<GraphExpandInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::expand(service.runtime.clone(), actor, origin, input).await)
}

#[get(
	"/api/authorization/{tenant}/graph-operator-grants",
	name = "!authorization_graph_operator_grants",
	auth = "protected"
)]
pub async fn list_grants(
	#[inject] service: Depends<GraphManagement>,
	Path(tenant): Path<String>,
	Query(page): Query<GrantPage>,
) -> ViewResult<Response> {
	crate::http::json(api::list_grants(service.runtime.clone(), tenant, Query(page)).await)
}

#[post(
	"/api/authorization/{tenant}/graph-operator-grants",
	name = "!authorization_set_graph_operator_grant",
	auth = "protected"
)]
pub async fn set_grant(
	#[inject] service: Depends<GraphManagement>,
	Path(tenant): Path<String>,
	Json(input): Json<GraphOperatorGrantInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::set_grant(service.runtime.clone(), tenant, input).await)
}

#[post(
	"/federation/v0.1/scoped/graph",
	name = "graphmanagement-project",
	auth = "protected"
)]
pub async fn project(
	#[inject] service: Depends<GraphManagement>,
	request: Request,
	Json(input): Json<GraphRequest>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::project(service.runtime.clone(), request.headers.clone(), input).await)
}

use crate::authorization::identity::Actor;
