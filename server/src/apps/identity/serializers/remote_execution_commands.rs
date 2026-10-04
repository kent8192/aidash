use serde::Deserialize;
// Serializable remote execution commands contracts.

#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
	pub(crate) grant_id: Uuid,
	pub(crate) admission_id: Uuid,
	pub(crate) operation: String,
	pub(crate) data: Value,
}

use uuid::Uuid;

use serde_json::Value;
