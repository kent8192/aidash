//! Persistent agent_knowledge records.

use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "registry", table_name = "agent_knowledge")]
#[derive(Serialize, Deserialize)]
pub struct AgentKnowledge {
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(primary_key = true, field_type = "text", db_column = "agent_id")]
	pub agent_key: String,
	#[field(primary_key = true, field_type = "text")]
	pub agent_version: String,
	#[field]
	pub documents: Json<Value>,
}
