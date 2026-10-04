//! Persistent packages records.

use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "registry", table_name = "packages")]
#[derive(Serialize, Deserialize)]
pub struct Package {
	#[field(primary_key = true, field_type = "text")]
	pub id: String,
	#[field(primary_key = true, field_type = "text")]
	pub version: String,
	#[field]
	pub manifest: Json<Value>,
	#[field(field_type = "text")]
	pub digest: String,
	#[field(field_type = "text")]
	pub manifest_source: String,
}
