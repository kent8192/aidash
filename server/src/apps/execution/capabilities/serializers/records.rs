use serde::{Deserialize, Serialize};
// Serializable records contracts.
use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
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
crate::native_record!(Record {
	id,
	tenant,
	owner,
	area_id,
	kind,
	state,
	revision,
	data,
	expires_at
});

use uuid::Uuid;
