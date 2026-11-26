use super::super::{
	oidc::BrowserOrigin, repositories::embedding_settings::Update,
	services::tenant_administration::TenantAdministrations,
};
use crate::{authorization::identity::Actor, http::json::Json};
use reinhardt::{Depends, Path, Response, get, http::ViewResult, put};

#[get(
	"/api/tenants/{tenant}/embedding-settings",
	name = "tenant-embedding-settings",
	auth = "protected"
)]
pub async fn get(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.embedding_settings(actor, origin, tenant, None)
			.await,
	)
}

#[put(
	"/api/tenants/{tenant}/embedding-settings",
	name = "tenant-embedding-settings-update",
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
			.embedding_settings(actor, origin, tenant, Some(update))
			.await,
	)
}
