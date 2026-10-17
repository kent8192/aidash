//! The Tenant creation API, Tenant labels and Tenant Deletion.
use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::serializers::tenants::{
	LabelUpdate, TenantCreation, TenantDeletionRequest,
};
use crate::apps::identity::services::tenant_deletions::TenantDeletions;
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
	#[inject] actor: Actor,
	Query(page): Query<PageQuery>,
) -> ViewResult<Response> {
	crate::http::json(service.list(actor, page).await)
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

/// Creates a new Personal Tenant for a signed-in External Identity whose
/// Personal Tenant was deleted. The service authenticates the browser session.
#[post("/api/tenants/personal", name = "tenants-personal", auth = "protected")]
pub async fn recreate_personal(
	#[inject] service: Depends<Tenants>,
	request: Request,
) -> ViewResult<Response> {
	crate::http::json_status(
		service.recreate_personal(request.headers).await,
		StatusCode::CREATED,
	)
}

/// Records a Tenant Deletion (ADR 0060).
#[post(
	"/api/tenants/{tenant}/deletion",
	name = "tenants-delete",
	auth = "protected"
)]
pub async fn delete(
	#[inject] service: Depends<TenantDeletions>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(input): Json<TenantDeletionRequest>,
) -> ViewResult<Response> {
	crate::http::json(service.delete(actor, origin, tenant, input).await)
}

/// Restores a deleted Tenant within its grace period. The route boundary is
/// public: a deleted Tenant's Mapping cannot pass the usual authentication,
/// so the service authenticates the caller itself.
#[post(
	"/api/tenants/{tenant}/restore",
	name = "tenants-restore",
	auth = "protected"
)]
pub async fn restore(
	#[inject] service: Depends<TenantDeletions>,
	request: Request,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	crate::http::json(service.restore(request.headers, tenant).await)
}
