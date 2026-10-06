//! Persistent generation_policies records.

use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "execution", table_name = "generation_policies")]
#[derive(Serialize, Deserialize)]
pub struct GenerationPolicy {
	#[field(field_type = "text", primary_key = true)]
	pub tenant: String,
	#[field(primary_key = true, field_type = "text")]
	pub id: String,
	#[field]
	pub revision: i64,
	#[field]
	pub spec: Json<Value>,
	#[field(default = 0)]
	pub generated_count: i64,
	#[field(default = 0)]
	pub allocated_tokens: i64,
	#[field(default = 0)]
	pub allocated_compaction_calls: i64,
	#[field(default = 0)]
	pub allocated_embedding_calls: i64,
}

impl GenerationPolicy {
	pub fn tenant_record_id(&self) -> String {
		self.tenant.clone()
	}
}
