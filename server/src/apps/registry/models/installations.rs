//! Persistent installations records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "registry", table_name = "installations")]
#[derive(Serialize, Deserialize)]
pub struct Installation {
	#[field(primary_key = true, field_type = "text")]
	pub id: String,
	#[field(primary_key = true, field_type = "text")]
	pub version: String,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub config: Json<Value>,
	#[field(auto_now_add = true)]
	pub installed_at: DateTime<Utc>,
}
