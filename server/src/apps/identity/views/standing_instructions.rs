use super::super::{
	oidc::BrowserOrigin, serializers::standing_instructions::Update,
	services::tenant_administration::TenantAdministrations,
};
use crate::{authorization::identity::Actor, http::json::Json};
use reinhardt::{Depends, Path, Response, get, http::ViewResult, put};

#[get(
	"/api/tenants/{tenant}/instructions",
	name = "tenant-instructions",
	auth = "protected"
)]
pub async fn tenant(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.tenant_instructions(actor, origin, tenant, None)
			.await,
	)
}

#[put(
	"/api/tenants/{tenant}/instructions",
	name = "tenant-instructions-update",
	auth = "protected"
)]
pub async fn tenant_update(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(update): Json<Update>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.tenant_instructions(actor, origin, tenant, Some(update))
			.await,
	)
}

#[get(
	"/api/tenants/{tenant}/nodes/{node}/instructions",
	name = "node-instructions",
	auth = "protected"
)]
pub async fn node(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, node)): Path<(String, String)>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.node_instructions(actor, origin, tenant, node, None)
			.await,
	)
}

#[put(
	"/api/tenants/{tenant}/nodes/{node}/instructions",
	name = "node-instructions-update",
	auth = "protected"
)]
pub async fn node_update(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, node)): Path<(String, String)>,
	Json(update): Json<Update>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.node_instructions(actor, origin, tenant, node, Some(update))
			.await,
	)
}
