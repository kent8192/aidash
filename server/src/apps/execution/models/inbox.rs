//! Persistent inbox records.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "inbox")]
#[derive(Serialize, Deserialize)]
pub struct Inbox {
	#[field(primary_key = true)]
	pub event_id: uuid::Uuid,
	#[field(auto_now_add = true)]
	pub received_at: DateTime<Utc>,
}
