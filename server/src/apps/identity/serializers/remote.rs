use serde::Deserialize;
// Serializable contracts for authorization.

use schemars::JsonSchema;

#[derive(Deserialize, JsonSchema)]
#[schemars(rename = "AuthorizationRemoteVerifyInput")]
#[serde(deny_unknown_fields)]
pub(crate) struct VerifyInput {
	pub(crate) grant_id: Uuid,
}

use uuid::Uuid;

pub(crate) use aidash_domain::federation::execution::Description;
pub use aidash_domain::federation::execution::{PrepareInput, Prepared};
