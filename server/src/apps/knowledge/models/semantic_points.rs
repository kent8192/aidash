//! Persistent semantic_points records.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "knowledge", table_name = "semantic_points")]
#[derive(Serialize, Deserialize)]
pub struct SemanticPoint {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field]
	pub entry_id: uuid::Uuid,
	#[field(field_type = "text", null = true)]
	pub content_digest: Option<String>,
	#[field(field_type = "text")]
	pub collection: String,
	#[field(default = false)]
	pub retired: bool,
	#[field(field_type = "text", null = true)]
	pub last_error: Option<String>,
	#[field(null = true)]
	pub cleaned_at: Option<DateTime<Utc>>,
	#[field(auto_now_add = true)]
	pub next_attempt: DateTime<Utc>,
}

impl SemanticPoint {
	pub fn collection_record_id(&self) -> String {
		self.collection.clone()
	}
}
