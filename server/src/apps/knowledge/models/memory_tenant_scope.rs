//! Tenant-selected reach of built-in cross-Run memory.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "knowledge", table_name = "memory_tenant_scope")]
#[derive(Serialize, Deserialize)]
pub struct MemoryTenantScope {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub scope: String,
	#[field]
	pub revision: i64,
	#[field]
	pub updated_at: DateTime<Utc>,
}
