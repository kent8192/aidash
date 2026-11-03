//! Current Tenant Data Protection policy and the preview of a proposed policy.
use super::super::{oidc::BrowserOrigin, services::tenant_administration::TenantAdministrations};
use crate::{authorization::identity::Actor, http::json::Json};
use aidash_domain::data_protection::DataProtection;
use reinhardt::{Depends, Path, Response, get, http::ViewResult, post, put};

#[get(
	"/api/tenants/{tenant}/data-protection",
	name = "tenant-data-protection",
	auth = "protected"
)]
pub async fn get_policy(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	crate::http::json(service.data_protection(actor, origin, tenant, None).await)
}
#[put(
	"/api/tenants/{tenant}/data-protection",
	name = "tenant-data-protection-update",
	auth = "protected"
)]
pub async fn update_policy(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(policy): Json<DataProtection>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.data_protection(actor, origin, tenant, Some(policy))
			.await,
	)
}
#[post(
	"/api/tenants/{tenant}/data-protection/impact",
	name = "tenant-data-protection-impact",
	auth = "protected"
)]
pub async fn impact(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(policy): Json<DataProtection>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.data_protection_impact(actor, origin, tenant, policy)
			.await,
	)
}
