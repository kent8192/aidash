//! Invitations: issuing them for a Tenant, and opening and redeeming a link.
use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::serializers::invitations::{InvitationLink, InvitationRequest};
use crate::apps::identity::services::invitations::Invitations;
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::{Depends, Path, Request, Response, StatusCode, get, http::ViewResult, post};
use uuid::Uuid;

/// Responses carry an Invitation secret or Tenant details, so none is cached.
fn no_store(response: ViewResult<Response>) -> ViewResult<Response> {
	response.map(|r| r.with_header("Cache-Control", "no-store"))
}

#[post(
	"/api/tenants/{tenant}/invitations",
	name = "tenant-invitations-issue",
	auth = "protected"
)]
pub async fn issue(
	#[inject] service: Depends<Invitations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Json(input): Json<InvitationRequest>,
) -> ViewResult<Response> {
	no_store(crate::http::json_status(
		service.issue(actor, origin, tenant, input).await,
		StatusCode::CREATED,
	))
}

#[get(
	"/api/tenants/{tenant}/invitations",
	name = "tenant-invitations",
	auth = "protected"
)]
pub async fn list(
	#[inject] service: Depends<Invitations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	no_store(crate::http::json(service.list(actor, origin, tenant).await))
}

#[post(
	"/api/tenants/{tenant}/invitations/{id}/revoke",
	name = "tenant-invitations-revoke",
	auth = "protected"
)]
pub async fn revoke(
	#[inject] service: Depends<Invitations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, id)): Path<(String, Uuid)>,
) -> ViewResult<Response> {
	no_store(crate::http::json(
		service.revoke(actor, origin, tenant, id).await,
	))
}

/// Opening a link shows the inviting Tenant and spends nothing.
#[post(
	"/auth/invitations/inspect",
	name = "invitations-inspect",
	auth = "public"
)]
pub async fn inspect(
	#[inject] service: Depends<Invitations>,
	request: Request,
	Json(input): Json<InvitationLink>,
) -> ViewResult<Response> {
	no_store(crate::http::json(
		service.inspect(request.headers, input).await,
	))
}

/// Requesting to join spends the link on a Registration Request addressed to
/// its Tenant, which an administrator still approves.
#[post(
	"/auth/invitations/redeem",
	name = "invitations-redeem",
	auth = "public"
)]
pub async fn redeem(
	#[inject] service: Depends<Invitations>,
	request: Request,
	Json(input): Json<InvitationLink>,
) -> ViewResult<Response> {
	no_store(crate::http::json(
		service.redeem(request.headers, input).await,
	))
}
