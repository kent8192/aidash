//! Native HTTP endpoints.
use crate::apps::identity::peer::admission::PeerAdmissions;
use crate::apps::identity::serializers::peer_admission::Input;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Request;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::post;
use uuid::Uuid;

#[post(
	"/federation/v0.1/scoped/execution/admissions",
	name = "admissions-admit",
	auth = "protected"
)]
pub async fn admit(
	#[inject] service: Depends<PeerAdmissions>,
	request: Request,
	Json(input): Json<Input>,
) -> ViewResult<Response> {
	crate::http::json(service.admit(request.headers, input).await)
}

#[post(
	"/federation/v0.1/scoped/execution/admissions/{id}/verify",
	name = "admissions-verify",
	auth = "protected"
)]
pub async fn verify(
	#[inject] service: Depends<PeerAdmissions>,
	request: Request,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.verify(request.headers, id).await)
}
