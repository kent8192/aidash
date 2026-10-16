//! The Tenant creation API and Tenant labels.
use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::serializers::tenants::{LabelUpdate, TenantCreation};
use crate::apps::identity::services::tenants::Tenants;
use crate::apps::workspaces::serializers::tasks::PageQuery;
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::{
	Depends, Path, Query, Request, Response, StatusCode, get, http::ViewResult, post, put,
};

/// Creates a Tenant under an assigned name. The route boundary is public and
/// the service authenticates the Deployment Administrator bearer token or a
/// browser session without a Mapping; the bearer is what the contract declares.
#[post("/api/tenants", name = "tenants-create", auth = "protected")]
pub async fn create(
	#[inject] service: Depends<Tenants>,
	request: Request,
	Json(input): Json<TenantCreation>,
) -> ViewResult<Response> {
	crate::http::json_status(
		service.create(request.headers, input).await,
		StatusCode::CREATED,
	)
}

#[get("/api/tenants", name = "tenants-list", auth = "protected")]
pub async fn list(
	#[inject] service: Depends<Tenants>,
	Query(page): Query<PageQuery>,
) -> ViewResult<Response> {
	crate::http::json(service.list(page).await)
}

#[put(
	"/api/tenants/{tenant}/label",
	name = "tenants-label",
	auth = "protected"
)]
pub async fn relabel(
	#[inject] service: Depends<Tenants>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(input): Json<LabelUpdate>,
) -> ViewResult<Response> {
	crate::http::json(service.relabel(actor, origin, tenant, input).await)
}
