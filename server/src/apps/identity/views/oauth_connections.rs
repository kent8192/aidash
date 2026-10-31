//! OAuth Connection routes.
use super::super::{
	serializers::oauth_connections::{CallbackQuery, ProbeQuery, Start},
	services::{oauth_connections::Connections, oidc::BrowserOrigin},
};
use crate::{authorization::identity::Actor, http::json::Json};
use reinhardt::di::params::PathStruct;
use reinhardt::{Depends, Path, Query, Request, Response, delete, get, http::ViewResult, post};
use uuid::Uuid;
#[derive(serde::Deserialize)]
pub struct MemberConnectionPath {
	pub tenant: String,
	pub subject: String,
	pub id: Uuid,
}
fn json<T: serde::Serialize>(value: crate::Result<T>) -> ViewResult<Response> {
	crate::http::json(value).map(|r| r.with_header("Cache-Control", "no-store"))
}
#[get(
	"/api/tenants/{tenant}/connections",
	name = "connection-list",
	auth = "protected"
)]
pub async fn list(
	#[inject] service: Depends<Connections>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	json(service.list(actor, origin, tenant).await)
}
#[delete(
	"/api/tenants/{tenant}/connections/{id}",
	name = "connection-disconnect",
	auth = "protected"
)]
pub async fn disconnect(
	#[inject] service: Depends<Connections>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, id)): Path<(String, Uuid)>,
) -> ViewResult<Response> {
	json(service.disconnect(actor, origin, tenant, id).await)
}
#[get(
	"/api/tenants/{tenant}/members/{subject}/connections",
	name = "connection-members",
	auth = "protected"
)]
pub async fn members(
	#[inject] service: Depends<Connections>,
	#[inject] actor: Actor,
	Path((tenant, subject)): Path<(String, String)>,
) -> ViewResult<Response> {
	json(service.members(actor, tenant, subject).await)
}
#[delete(
	"/api/tenants/{tenant}/members/{subject}/connections/{id}",
	name = "connection-force-disconnect",
	auth = "protected"
)]
pub async fn force_disconnect(
	#[inject] service: Depends<Connections>,
	#[inject] actor: Actor,
	PathStruct(MemberConnectionPath {
		tenant,
		subject,
		id,
	}): PathStruct<MemberConnectionPath>,
) -> ViewResult<Response> {
	json(service.force_disconnect(actor, tenant, subject, id).await)
}
#[get(
	"/api/tenants/{tenant}/connections/probe",
	name = "connection-probe",
	auth = "protected"
)]
pub async fn probe(
	#[inject] service: Depends<Connections>,
	#[inject] actor: Actor,
	Path(tenant): Path<String>,
	Query(query): Query<ProbeQuery>,
) -> ViewResult<Response> {
	json(service.probe(actor, tenant, query.resource).await)
}
#[post(
	"/api/tenants/{tenant}/connections/authorizations",
	name = "connection-start",
	auth = "protected"
)]
pub async fn start(
	#[inject] service: Depends<Connections>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	request: Request,
	Path(tenant): Path<String>,
	Json(input): Json<Start>,
) -> ViewResult<Response> {
	crate::http::response(
		service
			.start(actor, origin, request.headers, tenant, input)
			.await,
	)
}
#[get(
	"/auth/connections/callback",
	name = "connection-callback",
	auth = "public"
)]
pub async fn callback(
	#[inject] service: Depends<Connections>,
	request: Request,
	Query(query): Query<CallbackQuery>,
) -> ViewResult<Response> {
	crate::http::response(service.callback(request.headers, query).await)
}
