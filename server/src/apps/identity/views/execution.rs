//! Native HTTP endpoints.
use crate::apps::identity::peer::execution::PeerExecution;
use crate::apps::identity::serializers::peer_execution::InspectInput;
use crate::apps::identity::services::http_auth::InboundCall;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::post;

#[post(
	"/federation/v0.1/scoped/execution/inspect",
	name = "execution-inspect",
	auth = "protected"
)]
pub async fn inspect(
	#[inject] service: Depends<PeerExecution>,
	#[inject] peer: InboundCall,
	Json(input): Json<InspectInput>,
) -> ViewResult<Response> {
	crate::http::json(service.inspect(peer.0, input).await)
}
