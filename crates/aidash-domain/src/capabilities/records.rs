//! Durable capability state is distinct from the native database row and API schema.
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
#[derive(Clone, Debug)]
pub struct Record {
	pub id: Uuid,
	pub tenant: String,
	pub owner: String,
	pub area_id: Option<Uuid>,
	pub kind: String,
	pub state: String,
	pub revision: i64,
	pub data: Value,
	pub expires_at: Option<DateTime<Utc>>,
}
