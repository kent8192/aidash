//! The endpoints that the GCIP sign-in page calls.
use aidash_server::authorization::services::boundary::AccessBoundary;
use aidash_server::federation::Federation;
use aidash_server::http::json::Json;
use aidash_server::{Error, Result};
use reinhardt::http::ViewResult;
use reinhardt::{Query, Request, Response, UnifiedRouter, Validate, get, post};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
pub struct TransactionQuery {
	pub state: String,
}

#[derive(Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Exchange {
	#[validate(length(min = 1, max = 256))]
	pub state: String,
	#[validate(length(min = 1, max = 16384))]
	pub id_token: String,
}

fn provider(
	runtime: &Federation,
) -> Result<&dyn aidash_server::authorization::sign_in::SignInProvider> {
	runtime.config.sign_in.as_deref().ok_or(Error::Unauthorized)
}

#[get(
	"/auth/gcip/transaction",
	name = "sessions-gcip-transaction",
	auth = "public"
)]
pub async fn transaction(
	#[inject] runtime: Federation,
	request: Request,
	Query(query): Query<TransactionQuery>,
) -> ViewResult<Response> {
	aidash_server::http::response(match provider(&runtime) {
		Ok(provider) => {
			provider
				.transaction(&runtime, request.headers, query.state)
				.await
		}
		Err(error) => Err(error),
	})
}

#[post(
	"/auth/gcip/exchange",
	name = "sessions-gcip-exchange",
	auth = "public"
)]
pub async fn exchange(
	#[inject] runtime: Federation,
	request: Request,
	Json(input): Json<Exchange>,
) -> ViewResult<Response> {
	let result = async {
		aidash_server::http::validate(&input)?;
		provider(&runtime)?
			.exchange(&runtime, request.headers, input.state, input.id_token)
			.await
	};
	aidash_server::http::response(result.await)
}

pub fn routes() -> UnifiedRouter {
	UnifiedRouter::new().server(|server| {
		server
			.endpoint(transaction)
			.with_route_middleware(AccessBoundary::public())
			.endpoint(exchange)
			.with_route_middleware(AccessBoundary::public())
	})
}
