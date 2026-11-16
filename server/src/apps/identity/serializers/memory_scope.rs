use aidash_domain::memory::SharingScope;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Settings {
	pub scope: SharingScope,
	pub revision: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Update {
	pub scope: SharingScope,
	pub expected_revision: i64,
}
