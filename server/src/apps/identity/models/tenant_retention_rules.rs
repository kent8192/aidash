//! A Tenant's own Retention Rules and the history of their changes (ADR 0045).
//! A Tenant without a row keeps the Deployment's periods.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

/// The periods one Tenant keeps, never longer than the Deployment's.
#[model(app_label = "identity", table_name = "tenant_retention_rules")]
#[derive(Serialize, Deserialize)]
pub struct TenantRetentionRule {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field]
	pub snapshot_after_run_seconds: i32,
	#[field]
	pub snapshot_max_age_seconds: i32,
	#[field]
	pub updated_at: DateTime<Utc>,
	/// The recorded [`Decider`](aidash_domain::retention::Decider).
	#[field(field_type = "text")]
	pub updated_by: String,
}

/// One change of a Tenant's periods; cleared periods have no values.
#[model(app_label = "identity", table_name = "tenant_retention_rule_history")]
#[derive(Serialize, Deserialize)]
pub struct TenantRetentionRuleHistory {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(null = true)]
	pub snapshot_after_run_seconds: Option<i32>,
	#[field(null = true)]
	pub snapshot_max_age_seconds: Option<i32>,
	#[field]
	pub changed_at: DateTime<Utc>,
	#[field(field_type = "text")]
	pub changed_by: String,
}
