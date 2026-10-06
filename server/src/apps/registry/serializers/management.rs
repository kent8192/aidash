//! Management API contracts.
use crate::domain::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstallInput {
	pub(crate) digest: String,
	#[serde(default = "empty_object")]
	pub(crate) config: Value,
}
