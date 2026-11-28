//! Native HTTP endpoints.
use crate::apps::identity::peer::execution::PeerExecution;
use crate::apps::identity::serializers::peer_execution::InspectInput;
use crate::apps::identity::services::http_auth::InboundCall;
use crate::federation::Federation;
use crate::http::json::Json;
use aidash_domain::federation::preview::{InspectRequest, TicketRequest};
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
	let result = service.inspect(peer.0, input).await;
	use aidash_domain::federation::execution::Refusal;
	if let Err(crate::Error::RemoteExecutionRefused(refusal @ Refusal::ReceiverRefused(reason))) =
		&result
	{
		return Response::new(reinhardt::StatusCode::FORBIDDEN)
			.with_header("x-aidash-execution-refusal", &refusal.header())
			.with_json(reason);
	}
	crate::http::json(result)
}

#[post(
	"/federation/v0.1/scoped/previews/tickets",
	name = "peer-preview-ticket",
	auth = "protected"
)]
pub async fn preview_ticket(
	#[inject] runtime: Federation,
	#[inject] peer: InboundCall,
	Json(input): Json<TicketRequest>,
) -> ViewResult<Response> {
	crate::http::json(
		crate::apps::execution::capabilities::services::federation_previews::issue(
			runtime, peer.0, input,
		)
		.await,
	)
}

#[post(
	"/federation/v0.1/scoped/previews/inspect",
	name = "peer-preview-inspect",
	auth = "protected"
)]
pub async fn preview_inspect(
	#[inject] runtime: Federation,
	#[inject] peer: InboundCall,
	Json(input): Json<InspectRequest>,
) -> ViewResult<Response> {
	crate::http::json(
		crate::apps::execution::capabilities::services::federation_previews::inspect(
			runtime, peer.0, input,
		)
		.await,
	)
}
