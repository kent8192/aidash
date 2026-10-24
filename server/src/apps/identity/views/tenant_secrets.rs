//! Tenant-scoped Tenant Secret endpoints; all responses forbid caching and
//! none returns a value (ADR 0051).
use super::super::{serializers::tenant_secrets::*, services::tenant_secrets::Management};
use crate::{authorization::identity::Actor, http::json::Json};
use reinhardt::{Depends, Path, Response, delete, get, http::ViewResult, put};
fn response<T: serde::Serialize>(result: crate::Result<T>) -> ViewResult<Response> {
	crate::http::json(result).map(|r| r.with_header("Cache-Control", "no-store"))
}
#[get(
	"/api/tenants/{tenant}/secrets",
	name = "tenant-secret-list",
	auth = "protected"
)]
pub async fn list(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	response(service.list(actor, tenant).await)
}
#[get(
	"/api/tenants/{tenant}/secrets/{name}",
	name = "tenant-secret-get",
	auth = "protected"
)]
pub async fn get_record(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path((tenant, name)): Path<(String, String)>,
) -> ViewResult<Response> {
	response(service.get(actor, tenant, name).await)
}
#[put(
	"/api/tenants/{tenant}/secrets/{name}",
	name = "tenant-secret-put",
	auth = "protected"
)]
pub async fn put_record(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path((tenant, name)): Path<(String, String)>,
	Json(input): Json<Write>,
) -> ViewResult<Response> {
	response(service.put(actor, tenant, name, input).await)
}
#[delete(
	"/api/tenants/{tenant}/secrets/{name}",
	name = "tenant-secret-delete",
	auth = "protected"
)]
pub async fn delete_record(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path((tenant, name)): Path<(String, String)>,
	Json(input): Json<Revision>,
) -> ViewResult<Response> {
	response(service.delete(actor, tenant, name, input).await)
}
