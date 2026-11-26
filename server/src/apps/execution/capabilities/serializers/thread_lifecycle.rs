//! The HTTP request for deleting a Thread.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct DeleteThread {
	pub idempotency_key: Uuid,
}
