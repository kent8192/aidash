//! Persistent human_requests records.

use crate::apps::execution::services::states::HumanRequestKind;
use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "execution", table_name = "human_requests")]
#[derive(Serialize, Deserialize)]
pub struct HumanRequest {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field]
	pub workspace_id: uuid::Uuid,
	#[field]
	pub run_id: uuid::Uuid,
	#[field(field_type = "text", max_length = 64)]
	pub kind: HumanRequestKind,
	#[field(field_type = "text")]
	pub prompt: String,
	#[field(null = true)]
	pub response: Option<Json<Value>>,
	#[field(field_type = "text")]
	pub request_key: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(field_type = "text", null = true)]
	pub answered_by: Option<String>,
}

impl HumanRequest {}
