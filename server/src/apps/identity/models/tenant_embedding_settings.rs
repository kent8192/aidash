//! Tenant-wide embedding selection and an optional pending replacement.
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "identity", table_name = "tenant_embedding_settings")]
#[derive(Serialize, Deserialize)]
pub struct TenantEmbeddingSettings {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field]
	pub active: Json<Value>,
	#[field(null = true)]
	pub pending: Option<Json<Value>>,
	#[field]
	pub revision: i64,
	#[field(auto_now_add = true)]
	pub next_attempt: chrono::DateTime<chrono::Utc>,
}
