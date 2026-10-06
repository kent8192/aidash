//! Persistent agent_incidents records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "registry", table_name = "agent_incidents")]
#[derive(Serialize, Deserialize)]
pub struct AgentIncident {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub agent_id: String,
	#[field(field_type = "text")]
	pub version: String,
	#[field(default = 1)]
	pub revision: i64,
	#[field(field_type = "text")]
	pub severity: String,
	#[field(field_type = "text")]
	pub status: String,
	#[field(default = false)]
	pub archived: bool,
	#[field(field_type = "text")]
	pub owner: String,
	#[field(field_type = "text")]
	pub notes: String,
	#[field]
	pub evidence: Json<Value>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(auto_now_add = true)]
	pub updated_at: DateTime<Utc>,
	#[field(null = true)]
	pub resolved_at: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub evidence_expires_at: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub evidence_expired_at: Option<DateTime<Utc>>,
}
