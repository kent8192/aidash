//! HTTP endpoints backed by injected application services.
use crate::apps::identity::services::http_auth::InboundCall;
use crate::authorization::peer::graph as api;
use crate::http::json::Json;
use api::*;
use reinhardt::http::ViewResult;
use reinhardt::{Depends, Query, Response, get, post};

#[get(
	"/api/federation/graph/peers",
	name = "!federated_graph_peers",
	auth = "protected"
)]
pub async fn peers(
	#[inject] service: Depends<GraphManagement>,
	#[inject] actor: Actor,
	Query(sender): Query<crate::apps::federation::peer::views::management::SendingNode>,
) -> ViewResult<Response> {
	crate::http::json(api::peers(service.runtime.clone(), actor, sender.node).await)
}

#[post(
	"/api/federation/graph",
	name = "!federated_graph_expand",
	auth = "protected"
)]
pub async fn expand(
	#[inject] service: Depends<GraphManagement>,
	#[inject] actor: Actor,
	Json(input): Json<GraphExpandInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::expand(service.runtime.clone(), actor, input).await)
}

#[post(
	"/federation/v0.1/scoped/graph",
	name = "graphmanagement-project",
	auth = "protected"
)]
pub async fn project(
	#[inject] service: Depends<GraphManagement>,
	#[inject] caller: InboundCall,
	Json(input): Json<GraphRequest>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::project(service.runtime.clone(), caller.0, input).await)
}

use crate::authorization::identity::Actor;
