//! Persistent authorization_bundles records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "identity", table_name = "authorization_bundles")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationBundle {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field]
	pub revision: i64,
	#[field]
	pub document: Json<Value>,
	#[field(auto_now_add = true)]
	pub updated_at: DateTime<Utc>,
}
