//! Existing Home query and leaf verification protocol boundaries.
use crate::apps::identity::services::http_auth::InboundCall;
use crate::{federation::Federation, http::json::Json, semantic::remote::Operation};
use reinhardt::{Response, http::ViewResult, post};

#[post(
	"/federation/v0.1/scoped/semantic/query",
	name = "peer-semantic-query",
	auth = "protected"
)]
pub async fn query(
	#[inject] runtime: Federation,
	#[inject] peer: InboundCall,
	Json(input): Json<Operation>,
) -> ViewResult<Response> {
	crate::http::json(crate::authorization::remote::semantic::search(runtime, peer.0, input).await)
}

#[post(
	"/federation/v0.1/scoped/semantic/verify-operation",
	name = "peer-semantic-verify",
	auth = "protected"
)]
pub async fn verify(
	#[inject] runtime: Federation,
	#[inject] peer: InboundCall,
	Json(input): Json<Operation>,
) -> ViewResult<Response> {
	crate::http::json(
		crate::authorization::peer::semantic::verify_operation(runtime, peer.0, input).await,
	)
}
