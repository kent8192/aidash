//! Existing peer Generation and usage protocol endpoints.
use crate::apps::identity::services::http_auth::InboundCall;
use crate::generation::{
	foreign,
	remote::{dispatch, protocol},
};
use crate::{federation::Federation, http::json::Json};
use reinhardt::{Response, http::ViewResult, post};

#[post(
	"/federation/v0.1/scoped/generation/cancel",
	name = "peer-cancel-generation",
	auth = "protected"
)]
pub async fn cancel_generation(
	#[inject] runtime: Federation,
	#[inject] peer: InboundCall,
	Json(input): Json<foreign::Reference>,
) -> ViewResult<Response> {
	crate::http::json(foreign::cancel_at(runtime, peer.0, input).await)
}

#[post(
	"/federation/v0.1/scoped/generation/describe",
	name = "peer-describe-generation",
	auth = "protected"
)]
pub async fn describe_generation(
	#[inject] runtime: Federation,
	#[inject] peer: InboundCall,
	Json(input): Json<foreign::Reference>,
) -> ViewResult<Response> {
	crate::http::json(foreign::describe(runtime, peer.0, input).await)
}

#[post(
	"/federation/v0.1/scoped/generation/prepare",
	name = "peer-prepare-generation",
	auth = "protected"
)]
pub async fn prepare_generation(
	#[inject] runtime: Federation,
	#[inject] peer: InboundCall,
	Json(input): Json<foreign::Reference>,
) -> ViewResult<Response> {
	crate::http::json(foreign::prepare(runtime, peer.0, input).await)
}

#[post(
	"/federation/v0.1/scoped/usage/reserve",
	name = "peer-reserve-usage",
	auth = "protected"
)]
pub async fn reserve_usage(
	#[inject] runtime: Federation,
	#[inject] peer: InboundCall,
	Json(input): Json<dispatch::Input>,
) -> ViewResult<Response> {
	crate::http::json(protocol::reserve(runtime, peer.0, input).await)
}

#[post(
	"/federation/v0.1/scoped/usage/verify",
	name = "peer-verify-usage",
	auth = "protected"
)]
pub async fn verify_usage(
	#[inject] runtime: Federation,
	#[inject] peer: InboundCall,
	Json(input): Json<dispatch::Input>,
) -> ViewResult<Response> {
	crate::http::json(protocol::verify(runtime, peer.0, input).await)
}

#[post(
	"/federation/v0.1/scoped/usage/finalize",
	name = "peer-finalize-usage",
	auth = "protected"
)]
pub async fn finalize_usage(
	#[inject] runtime: Federation,
	#[inject] peer: InboundCall,
	Json(input): Json<dispatch::FinalizeInput>,
) -> ViewResult<Response> {
	crate::http::json(protocol::finalize(runtime, peer.0, input).await)
}
