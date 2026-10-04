use serde::{Deserialize, Serialize};
// Serializable service contracts.

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cursor {
	pub(crate) area: uuid::Uuid,
	pub(crate) generation: i64,
	pub(crate) revision: i64,
	pub(crate) query: String,
	pub(crate) file: usize,
	pub(crate) line: usize,
}
