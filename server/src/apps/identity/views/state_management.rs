//! HTTP conversion for the current management contract.
use uuid::Uuid;

#[reinhardt::get(
	"/api/runs/{id}/management",
	name = "run-management-get",
	auth = "protected"
)]
pub async fn get(
	#[inject] runtime: crate::federation::Federation,
	#[inject] actor: crate::authorization::identity::Actor,
	reinhardt::Path(id): reinhardt::Path<Uuid>,
) -> reinhardt::http::ViewResult<reinhardt::Response> {
	crate::http::json(crate::authorization::execution::management::get(runtime, actor, id).await)
}
#[reinhardt::post(
	"/api/runs/{id}/management",
	name = "run-management-control",
	auth = "protected"
)]
pub async fn control(
	#[inject] runtime: crate::federation::Federation,
	#[inject] actor: crate::authorization::identity::Actor,
	reinhardt::Path(id): reinhardt::Path<Uuid>,
	crate::http::json::Json(input): crate::http::json::Json<
		crate::authorization::execution::management::RunManagementInput,
	>,
) -> reinhardt::http::ViewResult<reinhardt::Response> {
	crate::http::json(
		crate::authorization::execution::management::control(runtime, actor, id, input).await,
	)
}
