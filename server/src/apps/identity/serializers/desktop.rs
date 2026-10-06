//! Desktop protocol inputs retain the merged browser broker wire contracts.
pub use aidash_application::authorization::desktop::{
	Exchange, Renewal, Revocation, Start, Started, Tokens,
};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;
#[derive(Deserialize, JsonSchema)]
pub(crate) struct AuthorizationRequest {
	pub request: Uuid,
}
#[derive(Deserialize, JsonSchema)]
pub(crate) struct Consent {
	pub request: Uuid,
	pub csrf: String,
}
