//! HTTP conversion for the current management contract.
use uuid::Uuid;

#[reinhardt::post(
	"/api/tasks/{id}/remote-generation",
	name = "remote-generation-prepare",
	auth = "protected"
)]
pub async fn request(
	#[inject] runtime: crate::federation::Federation,
	#[inject] actor: crate::authorization::identity::Actor,
	reinhardt::Path(id): reinhardt::Path<Uuid>,
	crate::http::json::Json(input): crate::http::json::Json<crate::generation::foreign::Input>,
) -> reinhardt::http::ViewResult<reinhardt::Response> {
	crate::http::json(crate::generation::foreign::request(runtime, actor, id, input).await)
}
#[reinhardt::post(
	"/api/tasks/{id}/remote-generation/{intent}/cancel",
	name = "remote-generation-cancel",
	auth = "protected"
)]
pub async fn cancel(
	#[inject] runtime: crate::federation::Federation,
	#[inject] actor: crate::authorization::identity::Actor,
	reinhardt::Path((id, intent)): reinhardt::Path<(Uuid, Uuid)>,
) -> reinhardt::http::ViewResult<reinhardt::Response> {
	crate::http::json(crate::generation::foreign::cancel(runtime, actor, (id, intent)).await)
}
