//! Persistent agent_incident_events records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "registry", table_name = "agent_incident_events")]
#[derive(Serialize, Deserialize)]
pub struct AgentIncidentEvent {
	#[field(primary_key = true)]
	pub id: i64,
	#[field]
	pub incident_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub actor: String,
	#[field]
	pub change: Json<Value>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

impl AgentIncidentEvent {}
