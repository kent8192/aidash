//! Persistent agent_test_profiles records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "registry", table_name = "agent_test_profiles")]
#[derive(Serialize, Deserialize)]
pub struct AgentTestProfile {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field(primary_key = true, field_type = "text")]
	pub id: String,
	#[field(default = 1)]
	pub revision: i64,
	#[field(default = true)]
	pub enabled: bool,
	#[field]
	pub rules: Json<Value>,
	#[field(auto_now_add = true)]
	pub updated_at: DateTime<Utc>,
}
