//! Plan Connection endpoints: one subject's own Sign in with ChatGPT grant in
//! one Tenant (#159). Every endpoint needs the owner's dashboard or desktop
//! session.
use super::super::{
	serializers::plan_connections::{Complete, Disconnect, Start},
	services::{oidc::BrowserOrigin, plan_connections::PlanConnections},
};
use crate::{authorization::identity::Actor, http::json::Json};
use reinhardt::{Depends, Path, Request, Response, delete, get, http::ViewResult, post};

/// A Plan Connection response describes one subject's own grant, so no cache
/// may keep it.
fn json<T: serde::Serialize>(result: crate::Result<T>) -> ViewResult<Response> {
	crate::http::json(result).map(|r| r.with_header("Cache-Control", "no-store"))
}

#[get(
	"/api/tenants/{tenant}/plan-connections/{provider}",
	name = "plan-connection-status",
	auth = "protected"
)]
pub async fn status(
	#[inject] service: Depends<PlanConnections>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, provider)): Path<(String, String)>,
) -> ViewResult<Response> {
	json(service.status(actor, origin, tenant, provider).await)
}

#[post(
	"/api/tenants/{tenant}/plan-connections/{provider}/authorizations",
	name = "plan-authorization-start",
	auth = "protected"
)]
pub async fn start(
	#[inject] service: Depends<PlanConnections>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	request: Request,
	Path((tenant, provider)): Path<(String, String)>,
	Json(input): Json<Start>,
) -> ViewResult<Response> {
	crate::http::response(
		service
			.start(actor, origin, request.headers, tenant, provider, input)
			.await,
	)
}

#[post(
	"/api/tenants/{tenant}/plan-connections/{provider}/authorizations/complete",
	name = "plan-authorization-complete",
	auth = "protected"
)]
pub async fn complete(
	#[inject] service: Depends<PlanConnections>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, provider)): Path<(String, String)>,
	Json(input): Json<Complete>,
) -> ViewResult<Response> {
	crate::http::response(
		service
			.complete(actor, origin, tenant, provider, input)
			.await,
	)
}

#[delete(
	"/api/tenants/{tenant}/plan-connections/{provider}",
	name = "plan-connection-disconnect",
	auth = "protected"
)]
pub async fn disconnect(
	#[inject] service: Depends<PlanConnections>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, provider)): Path<(String, String)>,
	Json(input): Json<Disconnect>,
) -> ViewResult<Response> {
	json(
		service
			.disconnect(actor, origin, tenant, provider, input)
			.await,
	)
}

#[get(
	"/api/tenants/{tenant}/plan-connections/{provider}/models",
	name = "plan-connection-models",
	auth = "protected"
)]
pub async fn models(
	#[inject] service: Depends<PlanConnections>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, provider)): Path<(String, String)>,
) -> ViewResult<Response> {
	json(service.models(actor, origin, tenant, provider).await)
}
