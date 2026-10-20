//! Live peer dependency verification preserves the existing Federation wire contract.
use crate::apps::identity::services::http_auth::InboundCall;
use crate::apps::identity::services::peer::dependencies::{Input, PeerDependencies};
use crate::http::json::Json;
use reinhardt::{Depends, Request, Response, http::ViewResult, post};

#[post(
	"/federation/v0.1/scoped/dependencies/verify",
	name = "dependencies-verify",
	auth = "protected"
)]
pub async fn verify(
	#[inject] service: Depends<PeerDependencies>,
	#[inject] peer: InboundCall,
	Json(input): Json<Input>,
) -> ViewResult<Response> {
	crate::http::json(service.verify(peer.0, input).await)
}
