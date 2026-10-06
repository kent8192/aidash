//! Existing Home query and leaf verification protocol boundaries.
use crate::{federation::Federation, http::json::Json, semantic::remote::Operation};
use reinhardt::{Request, Response, http::ViewResult, post};

#[post(
	"/federation/v0.1/scoped/semantic/query",
	name = "peer-semantic-query",
	auth = "protected"
)]
pub async fn query(
	#[inject] runtime: Federation,
	request: Request,
	Json(input): Json<Operation>,
) -> ViewResult<Response> {
	crate::http::json(
		crate::authorization::remote::semantic::search(runtime, request.headers, input).await,
	)
}

#[post(
	"/federation/v0.1/scoped/semantic/verify-operation",
	name = "peer-semantic-verify",
	auth = "protected"
)]
pub async fn verify(
	#[inject] runtime: Federation,
	request: Request,
	Json(input): Json<Operation>,
) -> ViewResult<Response> {
	crate::http::json(
		crate::authorization::peer::semantic::verify_operation(runtime, request.headers, input)
			.await,
	)
}
