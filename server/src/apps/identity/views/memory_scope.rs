use super::super::{
	oidc::BrowserOrigin, serializers::memory_scope::Update,
	services::tenant_administration::TenantAdministrations,
};
use crate::{authorization::identity::Actor, http::json::Json};
use reinhardt::{Depends, Path, Response, get, http::ViewResult, put};

#[get(
	"/api/tenants/{tenant}/memory-scope",
	name = "tenant-memory-scope",
	auth = "protected"
)]
pub async fn get(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	crate::http::json(service.memory_scope(actor, origin, tenant, None).await)
}

#[put(
	"/api/tenants/{tenant}/memory-scope",
	name = "tenant-memory-scope-update",
	auth = "protected"
)]
pub async fn update(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(update): Json<Update>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.memory_scope(actor, origin, tenant, Some(update))
			.await,
	)
}
