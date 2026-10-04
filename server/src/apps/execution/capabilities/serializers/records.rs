use serde::{Deserialize, Serialize};
// Serializable records contracts.
use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(
	Clone, Debug, Serialize, Deserialize, sqlx::FromRow, schemars::JsonSchema, reinhardt::Validate,
)]
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

use uuid::Uuid;
