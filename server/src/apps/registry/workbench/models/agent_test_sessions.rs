//! Persistent agent_test_sessions records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "registry", table_name = "agent_test_sessions")]
#[derive(Serialize, Deserialize)]
pub struct AgentTestSession {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field]
	pub draft_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub revision: i64,
	#[field(field_type = "text")]
	pub status: String,
	#[field]
	pub scenario: Json<Value>,
	#[field(null = true)]
	pub conversation: Option<Json<Value>>,
	#[field(null = true)]
	pub tool_calls: Option<Json<Value>>,
	#[field]
	pub usage: Json<Value>,
	#[field(field_type = "text", null = true)]
	pub error: Option<String>,
	#[field(null = true)]
	pub active_slot: Option<uuid::Uuid>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(auto_now_add = true)]
	pub updated_at: DateTime<Utc>,
	#[field]
	pub expires_at: DateTime<Utc>,
	#[field(null = true)]
	pub expired_at: Option<DateTime<Utc>>,
}

impl AgentTestSession {}
