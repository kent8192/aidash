//! Tenant Secret metadata (ADR 0051). The value is never an ORM field; it
//! stays in the Secret Store under the row's `secret_resource`.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One Tenant Secret of one Tenant. A name is unique among the Tenant's
/// Tenant Secrets that are not deleted.
#[model(app_label = "identity", table_name = "tenant_secrets")]
#[derive(Serialize, Deserialize)]
pub struct TenantSecret {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub name: String,
	/// The hosts the value may be sent to.
	#[field]
	pub hosts: Vec<String>,
	#[field(field_type = "text")]
	pub secret_resource: String,
	#[field(field_type = "text", null = true)]
	pub pinned_version: Option<String>,
	#[field(field_type = "text")]
	pub fingerprint: String,
	#[field(field_type = "text")]
	pub last4: String,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field]
	pub updated_at: DateTime<Utc>,
	#[field]
	pub revision: i64,
}
