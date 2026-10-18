//! Tenant Deletions (ADR 0060). Deployment-wide, one row per recorded
//! deletion: a restore closes the row, and a later deletion opens another.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "tenant_deletions")]
#[derive(Serialize, Deserialize)]
pub struct TenantDeletion {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub deleted_at: DateTime<Utc>,
	/// `tenant_administrator` or `deployment_administrator`.
	#[field(field_type = "text")]
	pub deleted_by_kind: String,
	/// The deleting External Identity; absent for the Deployment Administrator
	/// bearer token.
	#[field(null = true)]
	pub deleted_by_identity: Option<uuid::Uuid>,
	#[field]
	pub purge_after: DateTime<Utc>,
	#[field(null = true)]
	pub restored_at: Option<DateTime<Utc>>,
	#[field(field_type = "text", null = true)]
	pub restored_by_kind: Option<String>,
	#[field(null = true)]
	pub restored_by_identity: Option<uuid::Uuid>,
	#[field(null = true)]
	pub purged_at: Option<DateTime<Utc>>,
}
