//! HTTP conversion for the current management contract.
use uuid::Uuid;

#[reinhardt::get(
	"/api/tasks/{task}/remote-grants/{id}/semantic",
	name = "remote-semantic-home-provenance",
	auth = "protected"
)]
pub async fn provenance(
	#[inject] runtime: crate::federation::Federation,
	#[inject] actor: crate::authorization::identity::Actor,
	reinhardt::Path((task, id)): reinhardt::Path<(Uuid, Uuid)>,
) -> reinhardt::http::ViewResult<reinhardt::Response> {
	crate::http::json(
		crate::authorization::remote::execution::provenance(runtime, actor, (task, id)).await,
	)
}
#[reinhardt::post(
	"/api/tasks/{task}/remote-grants/{id}/follow-up",
	name = "remote-execution-follow-up",
	auth = "protected"
)]
pub async fn follow_up(
	#[inject] runtime: crate::federation::Federation,
	#[inject] actor: crate::authorization::identity::Actor,
	reinhardt::Path((task, id)): reinhardt::Path<(Uuid, Uuid)>,
	crate::http::json::Json(input): crate::http::json::Json<
		crate::authorization::remote::execution::FollowUpInput,
	>,
) -> reinhardt::http::ViewResult<reinhardt::Response> {
	crate::http::json(
		crate::authorization::remote::execution::follow_up(runtime, actor, (task, id), input).await,
	)
}
