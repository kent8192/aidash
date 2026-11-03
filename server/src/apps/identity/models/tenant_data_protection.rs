//! Current Tenant policy and the append-only history of its changes.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "tenant_data_protection")]
#[derive(Serialize, Deserialize)]
pub struct TenantDataProtection {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub level: String,
	#[field]
	pub model_call_zdr: bool,
	#[field]
	pub plan_connection_exempt: bool,
	#[field]
	pub updated_at: DateTime<Utc>,
	#[field(field_type = "text")]
	pub updated_by: String,
}

#[model(app_label = "identity", table_name = "tenant_data_protection_history")]
#[derive(Serialize, Deserialize)]
pub struct TenantDataProtectionHistory {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub level: String,
	#[field]
	pub model_call_zdr: bool,
	#[field]
	pub plan_connection_exempt: bool,
	#[field]
	pub changed_at: DateTime<Utc>,
	#[field(field_type = "text")]
	pub changed_by: String,
}
