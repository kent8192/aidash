//! Public receiving-key administration endpoints; no discovery HTTP is required.
use crate::apps::identity::{
	oidc::BrowserOrigin, serializers::manual_keys::*, services::manual_keys::ManualKeys,
};
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::{Depends, Path, Query, Response, get, http::ViewResult, post};

#[get(
	"/api/tenants/{tenant}/nodes/{node}/registration-keys",
	name = "tenant-registration-keys",
	auth = "protected"
)]
pub async fn registration_status(
	#[inject] service: Depends<ManualKeys>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, node)): Path<(String, String)>,
	Query(selector): Query<RegistrationKeySelector>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.registration(actor, origin, tenant, node, selector.source_node, None)
			.await,
	)
}
#[post(
	"/api/tenants/{tenant}/nodes/{node}/registration-keys",
	name = "tenant-registration-keys-change",
	auth = "protected"
)]
pub async fn registration_change(
	#[inject] service: Depends<ManualKeys>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, node)): Path<(String, String)>,
	Json(input): Json<RegistrationKeyChange>,
) -> ViewResult<Response> {
	let (source, change) = input.into_parts();
	crate::http::json(
		service
			.registration(actor, origin, tenant, node, source, Some(change))
			.await,
	)
}
#[get(
	"/api/authorization/{tenant}/peer-graph-grant-keys",
	name = "peer-graph-grant-keys",
	auth = "protected"
)]
pub async fn graph_status(
	#[inject] service: Depends<ManualKeys>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Query(selector): Query<GraphKeySelector>,
) -> ViewResult<Response> {
	crate::http::json(service.graph(actor, origin, tenant, selector, None).await)
}
#[post(
	"/api/authorization/{tenant}/peer-graph-grant-keys",
	name = "peer-graph-grant-keys-change",
	auth = "protected"
)]
pub async fn graph_change(
	#[inject] service: Depends<ManualKeys>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(input): Json<GraphKeyChange>,
) -> ViewResult<Response> {
	let (selector, change) = input.into_parts();
	crate::http::json(
		service
			.graph(actor, origin, tenant, selector, Some(change))
			.await,
	)
}
