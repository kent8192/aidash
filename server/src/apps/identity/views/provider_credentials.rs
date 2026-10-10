//! Tenant-scoped Provider Credential endpoints; all responses forbid caching.
use super::super::{
	serializers::provider_credentials::*, services::provider_credentials::Management,
};
use crate::{authorization::identity::Actor, http::json::Json};
use reinhardt::{Depends, Path, Query, Response, delete, get, http::ViewResult, post, put};
fn response<T: serde::Serialize>(result: crate::Result<T>) -> ViewResult<Response> {
	crate::http::json(result).map(|r| r.with_header("Cache-Control", "no-store"))
}
#[get(
	"/api/tenants/{tenant}/provider-credentials",
	name = "provider-credential-list",
	auth = "protected"
)]
pub async fn list(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path(tenant): Path<String>,
	Query(page): Query<Page>,
) -> ViewResult<Response> {
	response(service.list(actor, tenant, page).await)
}
#[get(
	"/api/tenants/{tenant}/provider-credentials/{id}",
	name = "provider-credential-get",
	auth = "protected"
)]
pub async fn get_record(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path((tenant, id)): Path<(String, uuid::Uuid)>,
) -> ViewResult<Response> {
	response(service.get(actor, tenant, id).await)
}
#[post(
	"/api/tenants/{tenant}/provider-credentials/{id}/revoke",
	name = "provider-credential-revoke",
	auth = "protected"
)]
pub async fn revoke(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path((tenant, id)): Path<(String, uuid::Uuid)>,
	Json(input): Json<Revision>,
) -> ViewResult<Response> {
	response(service.control(actor, tenant, id, input, false).await)
}
#[delete(
	"/api/tenants/{tenant}/provider-credentials/{id}",
	name = "provider-credential-delete",
	auth = "protected"
)]
pub async fn delete_record(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path((tenant, id)): Path<(String, uuid::Uuid)>,
	Json(input): Json<Revision>,
) -> ViewResult<Response> {
	response(service.control(actor, tenant, id, input, true).await)
}
#[get(
	"/api/tenants/{tenant}/provider-credential-bindings",
	name = "provider-credential-binding-list",
	auth = "protected"
)]
pub async fn list_bindings(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	response(service.bindings(actor, tenant, None).await)
}
#[get(
	"/api/tenants/{tenant}/provider-credential-bindings/{provider}",
	name = "provider-credential-binding-get",
	auth = "protected"
)]
pub async fn get_binding(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path((tenant, provider)): Path<(String, String)>,
) -> ViewResult<Response> {
	response(service.get_binding(actor, tenant, provider).await)
}
#[put(
	"/api/tenants/{tenant}/provider-credential-bindings/{provider}",
	name = "provider-credential-binding-update",
	auth = "protected"
)]
pub async fn update_binding(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path((tenant, provider)): Path<(String, String)>,
	Json(input): Json<BindingUpdate>,
) -> ViewResult<Response> {
	response(service.bind(actor, tenant, provider, input).await)
}
