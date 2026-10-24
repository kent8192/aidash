//! HTTP conversion for the current management contract.
use uuid::Uuid;

#[reinhardt::get(
	"/api/runs/{id}/semantic",
	name = "remote-semantic-run-provenance",
	auth = "protected"
)]
pub async fn run_receipt(
	#[inject] runtime: crate::federation::Federation,
	#[inject] actor: crate::authorization::identity::Actor,
	reinhardt::Path(id): reinhardt::Path<Uuid>,
) -> reinhardt::http::ViewResult<reinhardt::Response> {
	crate::http::json(crate::semantic::remote::status::run_receipt(runtime, actor, id).await)
}
