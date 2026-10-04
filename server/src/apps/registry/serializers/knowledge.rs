use serde::Deserialize;
// Serializable contracts for registry.

use crate::registry::Entry;
use schemars::JsonSchema;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PersonalAgent {
	pub entry: Entry,
	pub documents: Vec<ReferenceDocument>,
}

pub use aidash_domain::registry::ReferenceDocument;
