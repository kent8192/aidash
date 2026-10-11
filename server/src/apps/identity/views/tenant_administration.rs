//! Tenant Administrator endpoints. Responses carry Display Attributes, so
//! every response forbids caching.
use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::serializers::oidc::{AdminMappingPage, MappingRevision};
use crate::apps::identity::serializers::tenant_administration::{MembershipUpdate, TenantApproval};
use crate::apps::identity::services::tenant_administration::TenantAdministrations;
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::{Depends, Path, Query, Response, get, http::ViewResult, post, put};
use uuid::Uuid;

fn response<T: serde::Serialize>(result: crate::Result<T>) -> ViewResult<Response> {
	crate::http::json(result).map(|r| r.with_header("Cache-Control", "no-store"))
}

#[get(
	"/api/tenants/{tenant}/administration",
	name = "tenant-administration",
	auth = "protected"
)]
pub async fn overview(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	response(service.overview(actor, origin, tenant).await)
}

#[get(
	"/api/tenants/{tenant}/registrations",
	name = "tenant-registrations",
	auth = "protected"
)]
pub async fn registrations(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	response(service.registrations(actor, origin, tenant).await)
}

#[post(
	"/api/tenants/{tenant}/registrations/{id}/approve",
	name = "tenant-registration-approve",
	auth = "protected"
)]
pub async fn approve(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, id)): Path<(String, Uuid)>,
	Json(input): Json<TenantApproval>,
) -> ViewResult<Response> {
	response(service.approve(actor, origin, tenant, id, input).await)
}

#[post(
	"/api/tenants/{tenant}/registrations/{id}/reject",
	name = "tenant-registration-reject",
	auth = "protected"
)]
pub async fn reject(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, id)): Path<(String, Uuid)>,
) -> ViewResult<Response> {
	response(service.reject(actor, origin, tenant, id).await)
}

#[get(
	"/api/tenants/{tenant}/mappings",
	name = "tenant-mappings",
	auth = "protected"
)]
pub async fn mappings(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Query(page): Query<AdminMappingPage>,
) -> ViewResult<Response> {
	response(service.mappings(actor, origin, tenant, page.offset).await)
}

#[post(
	"/api/tenants/{tenant}/mappings/{id}/disable",
	name = "tenant-mapping-disable",
	auth = "protected"
)]
pub async fn disable_mapping(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, id)): Path<(String, Uuid)>,
	Json(input): Json<MappingRevision>,
) -> ViewResult<Response> {
	crate::http::status(
		service
			.disable_mapping(actor, origin, tenant, id, input)
			.await,
	)
	.map(|r| r.with_header("Cache-Control", "no-store"))
}

#[put(
	"/api/tenants/{tenant}/subjects/{subject}/groups",
	name = "tenant-subject-groups",
	auth = "protected"
)]
pub async fn update_memberships(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, subject)): Path<(String, String)>,
	Json(input): Json<MembershipUpdate>,
) -> ViewResult<Response> {
	response(
		service
			.update_memberships(actor, origin, tenant, subject, input)
			.await,
	)
}
