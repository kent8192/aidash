//! The idempotency key for a Thread deletion.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteThread {
	pub idempotency_key: Uuid,
}
