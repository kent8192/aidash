//! Persistent generation_policy_history records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "execution", table_name = "generation_policy_history")]
#[derive(Serialize, Deserialize)]
pub struct GenerationPolicyHistory {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(primary_key = true, field_type = "text", db_column = "policy_id")]
	pub policy_key: String,
	#[field(primary_key = true)]
	pub revision: i64,
	#[field]
	pub spec: Json<Value>,
	#[field(field_type = "text")]
	pub actor: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}
