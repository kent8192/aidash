//! OAuth Client and connection scope ceiling endpoints.
use super::super::{
	serializers::oauth_clients::{CeilingWrite, Write},
	services::oauth_clients::Management,
};
use crate::{authorization::identity::Actor, http::json::Json};
use reinhardt::{Depends, Path, Response, delete, get, http::ViewResult, put};
use uuid::Uuid;
fn response<T: serde::Serialize>(result: crate::Result<T>) -> ViewResult<Response> {
	crate::http::json(result).map(|response| response.with_header("Cache-Control", "no-store"))
}
#[get(
	"/api/tenants/{tenant}/oauth-clients",
	name = "oauth-clients-list",
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
	"/api/tenants/{tenant}/oauth-clients/{id}",
	name = "oauth-clients-get-record",
	auth = "protected"
)]
pub async fn get_record(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path((tenant, id)): Path<(String, Uuid)>,
) -> ViewResult<Response> {
	response(service.get(actor, tenant, id).await)
}
#[put(
	"/api/tenants/{tenant}/oauth-clients",
	name = "oauth-clients-put-collection",
	auth = "protected"
)]
pub async fn put_collection(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path(tenant): Path<String>,
	Json(input): Json<Write>,
) -> ViewResult<Response> {
	response(service.put(actor, tenant, None, input).await)
}
#[put(
	"/api/tenants/{tenant}/oauth-clients/{id}",
	name = "oauth-clients-put-record",
	auth = "protected"
)]
pub async fn put_record(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path((tenant, id)): Path<(String, Uuid)>,
	Json(input): Json<Write>,
) -> ViewResult<Response> {
	response(service.put(actor, tenant, Some(id), input).await)
}
#[delete(
	"/api/tenants/{tenant}/oauth-clients/{id}",
	name = "oauth-clients-delete-record",
	auth = "protected"
)]
pub async fn delete_record(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path((tenant, id)): Path<(String, Uuid)>,
) -> ViewResult<Response> {
	response(service.delete(actor, tenant, id).await)
}
#[get(
	"/api/tenants/{tenant}/connection-scope-ceilings/{resource}",
	name = "oauth-clients-ceiling",
	auth = "protected"
)]
pub async fn ceiling(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path((tenant, resource)): Path<(String, String)>,
) -> ViewResult<Response> {
	response(service.ceiling(actor, tenant, resource).await)
}
#[put(
	"/api/tenants/{tenant}/connection-scope-ceilings/{resource}",
	name = "oauth-clients-put-ceiling",
	auth = "protected"
)]
pub async fn put_ceiling(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path((tenant, resource)): Path<(String, String)>,
	Json(input): Json<CeilingWrite>,
) -> ViewResult<Response> {
	response(service.put_ceiling(actor, tenant, resource, input).await)
}
#[delete(
	"/api/tenants/{tenant}/connection-scope-ceilings/{resource}",
	name = "oauth-clients-delete-ceiling",
	auth = "protected"
)]
pub async fn delete_ceiling(
	#[inject] service: Depends<Management>,
	#[inject] actor: Actor,
	Path((tenant, resource)): Path<(String, String)>,
) -> ViewResult<Response> {
	response(service.delete_ceiling(actor, tenant, resource).await)
}
