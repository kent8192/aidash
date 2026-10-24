//! Node Deletions (ADR 0083). Tenant-owned, one row per recorded
//! deletion: a restore closes the row, and a later deletion opens another.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "node_deletions")]
#[derive(Serialize, Deserialize)]
pub struct NodeDeletion {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub node: String,
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
	/// The purge step that runs next, or keeps failing; absent before the first
	/// attempt and once the purge finished.
	#[field(field_type = "text", null = true)]
	pub purge_step: Option<String>,
	/// Consecutive failures of `purge_step`.
	#[field]
	pub purge_failures: i64,
	#[field(field_type = "text", null = true)]
	pub purge_error: Option<String>,
	#[field(null = true)]
	pub purge_failed_at: Option<DateTime<Utc>>,
	/// The purge worker that owns this purge, so replicas do not race. Every
	/// destructive step locks the row and checks it still holds this token.
	#[field(null = true)]
	pub purge_lease: Option<uuid::Uuid>,
	/// Until when that worker owns it.
	#[field(null = true)]
	pub purge_leased_until: Option<DateTime<Utc>>,
	/// When the audit history that outlived the purge was deleted.
	#[field(null = true)]
	pub audit_erased_at: Option<DateTime<Utc>>,
}
