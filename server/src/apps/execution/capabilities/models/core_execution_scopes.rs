//! Persistent core_execution_scopes records.
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "execution", table_name = "core_execution_scopes")]
#[derive(Serialize, Deserialize)]
pub struct CoreExecutionScopes {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub home_node: String,
	#[field(field_type = "text")]
	pub agent_id: String,
	#[field(field_type = "text")]
	pub owner: String,
	#[field]
	pub thread_id: uuid::Uuid,
	#[field]
	pub generation: i64,
	#[field]
	pub revision: i64,
	#[field]
	pub epoch: i64,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub constraints: Json<Value>,
}
