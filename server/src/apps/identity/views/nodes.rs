//! A Tenant's Nodes (#241).
use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::serializers::nodes::{
	NodeCreation, NodeDeletionRequest, NodeLabelUpdate, NodeList,
};
use crate::apps::identity::services::nodes::Nodes;
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::{Depends, Path, Query, Response, StatusCode, get, http::ViewResult, patch, post};

#[get(
	"/api/tenants/{tenant}/nodes",
	name = "tenant-nodes",
	auth = "protected"
)]
pub async fn list(
	#[inject] service: Depends<Nodes>,
	#[inject] actor: Actor,
	Path(tenant): Path<String>,
	Query(page): Query<NodeList>,
) -> ViewResult<Response> {
	crate::http::json(service.list(actor, tenant, page).await)
}

#[post(
	"/api/tenants/{tenant}/nodes",
	name = "tenant-nodes-create",
	auth = "protected"
)]
pub async fn create(
	#[inject] service: Depends<Nodes>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(input): Json<NodeCreation>,
) -> ViewResult<Response> {
	crate::http::json_status(
		service.create(actor, origin, tenant, input).await,
		StatusCode::CREATED,
	)
}

#[patch(
	"/api/tenants/{tenant}/nodes/{node}",
	name = "tenant-node-update",
	auth = "protected"
)]
pub async fn relabel(
	#[inject] service: Depends<Nodes>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, node)): Path<(String, String)>,
	Json(input): Json<NodeLabelUpdate>,
) -> ViewResult<Response> {
	crate::http::json(service.relabel(actor, origin, tenant, node, input).await)
}

#[post(
	"/api/tenants/{tenant}/nodes/{node}/default",
	name = "tenant-node-default",
	auth = "protected"
)]
pub async fn make_default(
	#[inject] service: Depends<Nodes>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, node)): Path<(String, String)>,
) -> ViewResult<Response> {
	crate::http::json(service.make_default(actor, origin, tenant, node).await)
}

#[post(
	"/api/tenants/{tenant}/nodes/{node}/deletion",
	name = "tenant-node-deletion",
	auth = "protected"
)]
pub async fn delete(
	#[inject] service: Depends<Nodes>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, node)): Path<(String, String)>,
	Json(input): Json<NodeDeletionRequest>,
) -> ViewResult<Response> {
	crate::http::json(service.delete(actor, origin, tenant, node, input).await)
}

#[post(
	"/api/tenants/{tenant}/nodes/{node}/restore",
	name = "tenant-node-restore",
	auth = "protected"
)]
pub async fn restore(
	#[inject] service: Depends<Nodes>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, node)): Path<(String, String)>,
) -> ViewResult<Response> {
	crate::http::json(service.restore(actor, origin, tenant, node).await)
}

#[post(
	"/api/tenants/{tenant}/nodes/{node}/purge",
	name = "tenant-node-purge",
	auth = "protected"
)]
pub async fn purge(
	#[inject] service: Depends<Nodes>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, node)): Path<(String, String)>,
) -> ViewResult<Response> {
	crate::http::json(service.purge(actor, origin, tenant, node).await)
}
