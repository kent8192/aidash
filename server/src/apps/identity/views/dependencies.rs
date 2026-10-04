//! Live peer dependency verification preserves the existing Federation wire contract.
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
	request: Request,
	Json(input): Json<Input>,
) -> ViewResult<Response> {
	crate::http::json(service.verify(request.headers, input).await)
}
