//! Native HTTP endpoints.
use crate::apps::identity::peer::execution::PeerExecution;
use crate::apps::identity::serializers::peer_execution::InspectInput;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Request;
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
	request: Request,
	Json(input): Json<InspectInput>,
) -> ViewResult<Response> {
	crate::http::json(service.inspect(request.headers, input).await)
}
