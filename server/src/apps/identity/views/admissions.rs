//! Native HTTP endpoints.
use crate::apps::identity::peer::admission::PeerAdmissions;
use crate::apps::identity::serializers::peer_admission::Input;
use crate::apps::identity::services::http_auth::InboundCall;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
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
	#[inject] peer: InboundCall,
	Json(input): Json<Input>,
) -> ViewResult<Response> {
	crate::http::json(service.admit(peer.0, input).await)
}

#[post(
	"/federation/v0.1/scoped/execution/admissions/{id}/verify",
	name = "admissions-verify",
	auth = "protected"
)]
pub async fn verify(
	#[inject] service: Depends<PeerAdmissions>,
	#[inject] peer: InboundCall,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.verify(peer.0, id).await)
}
