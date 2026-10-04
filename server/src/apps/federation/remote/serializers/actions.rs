//! API response contracts.
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Serialize, JsonSchema)]
pub struct SentResponse {
	pub sent: bool,
}
