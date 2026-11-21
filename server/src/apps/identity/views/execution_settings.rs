use super::super::{
	oidc::BrowserOrigin, serializers::execution_settings::TenantExecutionSettingsUpdate,
	services::tenant_administration::TenantAdministrations,
};
use crate::{authorization::identity::Actor, http::json::Json};
use reinhardt::{Depends, Path, Response, get, http::ViewResult, put};

#[get(
	"/api/tenants/{tenant}/execution-settings",
	name = "tenant-execution-settings",
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
			.execution_settings(actor, origin, tenant, None)
			.await,
	)
}

#[put(
	"/api/tenants/{tenant}/execution-settings",
	name = "tenant-execution-settings-update",
	auth = "protected"
)]
pub async fn update(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(update): Json<TenantExecutionSettingsUpdate>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.execution_settings(actor, origin, tenant, Some(update))
			.await,
	)
}
