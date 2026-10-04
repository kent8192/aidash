use serde::Deserialize;
// Serializable contracts for authorization.

use schemars::JsonSchema;

pub(crate) use aidash_domain::federation::registry_reads::Reference;

#[derive(Deserialize, JsonSchema)]
#[schemars(rename = "AuthorizationPeerReadsVerifyInput")]
#[serde(deny_unknown_fields)]
pub(crate) struct VerifyInput {
	pub(crate) tenant: String,
	pub(crate) subject: String,
	pub(crate) references: Vec<Reference>,
}
