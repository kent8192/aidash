//! Request and response contracts.
use crate::apps::execution::generation::policy::Spec;
use schemars::JsonSchema;
use serde::Deserialize;
#[derive(Deserialize, JsonSchema)]
#[schemars(rename = "GenerationPolicyUpdate")]
#[serde(deny_unknown_fields)]
pub struct PolicyUpdate {
	pub(crate) expected_revision: i64,
	pub(crate) spec: Spec,
}
#[derive(Deserialize, JsonSchema)]
#[schemars(rename = "GenerationAssignInput")]
#[serde(deny_unknown_fields)]
pub struct AssignInput {
	pub(crate) policy_id: String,
	pub(crate) reason: String,
}
pub use aidash_domain::generation::requests::Usage;
