//! Persistent registry_requests records.

use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "registry", table_name = "registry_requests")]
#[derive(Serialize, Deserialize)]
pub struct RegistryRequest {
	#[field(primary_key = true)]
	pub key: uuid::Uuid,
	#[field]
	pub request: Json<Value>,
	#[field(field_type = "text")]
	pub entity_id: String,
}
