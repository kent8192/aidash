//! Native HTTP endpoints.
use crate::apps::identity::peer::execution::PeerExecution;
use crate::apps::identity::serializers::peer_execution::InspectInput;
use crate::apps::identity::services::http_auth::InboundCall;
use crate::federation::Federation;
use crate::http::json::Json;
use aidash_domain::federation::preview::{
	InspectRequest, ReleaseRequest, RetainRequest, TicketRequest,
};
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

#[post(
	"/federation/v0.1/scoped/previews/retain",
	name = "peer-preview-retain",
	auth = "protected"
)]
pub async fn preview_retain(
	#[inject] runtime: Federation,
	#[inject] peer: InboundCall,
	Json(input): Json<RetainRequest>,
) -> ViewResult<Response> {
	crate::http::json(
		crate::apps::execution::capabilities::services::federation_previews::retain(
			runtime, peer.0, input,
		)
		.await,
	)
}

#[post(
	"/federation/v0.1/scoped/previews/release",
	name = "peer-preview-release",
	auth = "protected"
)]
pub async fn preview_release(
	#[inject] runtime: Federation,
	#[inject] peer: InboundCall,
	Json(input): Json<ReleaseRequest>,
) -> ViewResult<Response> {
	crate::http::json(
		crate::apps::execution::capabilities::services::federation_previews::release(
			runtime, peer.0, input,
		)
		.await,
	)
}
