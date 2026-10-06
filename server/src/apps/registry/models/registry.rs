//! Persistent registry records.

use crate::apps::registry::services::states::DefinitionKind;
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "registry", table_name = "registry")]
#[derive(Serialize, Deserialize)]
pub struct Definition {
	#[field(primary_key = true, field_type = "text")]
	pub id: String,
	#[field(primary_key = true, field_type = "text")]
	pub version: String,
	#[field(field_type = "text", max_length = 64)]
	pub kind: DefinitionKind,
	#[field]
	pub metadata: Json<Value>,
}
