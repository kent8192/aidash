//! A Tenant's execution settings (#1009).
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "tenant_execution_settings")]
#[derive(Serialize, Deserialize)]
pub struct TenantExecutionSettings {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	/// Consecutive Lease Recoveries before a Crash Loop Failure, 1 to 20.
	#[field]
	pub lease_recovery_limit: i32,
	#[field]
	pub revision: i64,
	#[field]
	pub updated_at: DateTime<Utc>,
	#[field(field_type = "text")]
	pub updated_by: String,
}
