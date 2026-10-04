use serde::{Deserialize, Serialize};
// Serializable thread lifecycle contracts.
use crate::apps::execution::capabilities::services::core::cleanup::Choice;

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct FileChoice {
	pub area_id: Uuid,
	pub expected_revision: i64,
	pub choice: Choice,
	pub confirmation_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct DeleteThread {
	pub idempotency_key: Uuid,
	pub files: Vec<FileChoice>,
}

use uuid::Uuid;
