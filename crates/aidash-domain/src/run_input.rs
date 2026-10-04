//! Input records replayed into the next inference context.
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct RunInput {
	pub seq: i64,
	pub sender: String,
	pub content: String,
	pub idempotency_key: String,
	pub message_id: Option<Uuid>,
	pub reference_only: bool,
}
