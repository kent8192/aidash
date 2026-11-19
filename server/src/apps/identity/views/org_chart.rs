//! The org chart and stopping a flow (#1012, ADR 0143). Both need a person's
//! browser session.
use super::super::{oidc::BrowserOrigin, services::tenant_administration::TenantAdministrations};
use crate::authorization::identity::Actor;
use reinhardt::{Depends, Path, Response, get, http::ViewResult, post};
use uuid::Uuid;

#[get(
	"/api/tenants/{tenant}/org-chart",
	name = "org-chart",
	auth = "protected"
)]
pub async fn chart(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	crate::http::json(service.org_chart(actor, origin, tenant).await)
}

#[post(
	"/api/tenants/{tenant}/flows/{root_task}/stop",
	name = "flow-stop",
	auth = "protected"
)]
pub async fn stop_flow(
	#[inject] service: Depends<TenantAdministrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, root_task)): Path<(String, Uuid)>,
) -> ViewResult<Response> {
	crate::http::json(service.stop_flow(actor, origin, tenant, root_task).await)
}
