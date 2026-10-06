//! Public API schema.
use crate::{apps::execution::services::schema::ApiContract, http::json};
use reinhardt::http::ViewResult;
use reinhardt::{Depends, Response, get};

#[get("/api/openapi.json", name = "openapi", auth = "public")]
pub async fn document(#[inject] contract: Depends<ApiContract>) -> ViewResult<Response> {
	json(contract.document())
}
