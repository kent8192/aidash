//! Native HTTP endpoints.
use crate::apps::identity::peer::reads::PeerReads;
use crate::apps::identity::serializers::peer_reads::VerifyInput;
use crate::apps::identity::services::http_auth::InboundCall;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::post;

#[post(
	"/federation/v0.1/scoped/registry/verify",
	name = "reads-verify",
	auth = "protected"
)]
pub async fn verify(
	#[inject] service: Depends<PeerReads>,
	#[inject] peer: InboundCall,
	Json(input): Json<VerifyInput>,
) -> ViewResult<Response> {
	crate::http::json(service.verify(peer.0, input).await)
}
