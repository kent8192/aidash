//! Contracts of the Tenant creation API, Tenant labels and Tenant Deletion.
use chrono::{DateTime, Utc};
use reinhardt::Validate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct TenantCreation {
	/// The Tenant label: 1–64 characters after trimming, without control characters.
	#[validate(length(min = 1, max = 256))]
	pub(crate) label: String,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct CreatedTenant {
	/// The Tenant name Aidash assigned.
	pub(crate) name: String,
	pub(crate) label: String,
	pub(crate) created_at: DateTime<Utc>,
	/// The Tenant Creator's Mapping; absent when a Deployment Administrator created it.
	pub(crate) mapping_id: Option<Uuid>,
}

/// A Tenant's root record as Deployment Administrators see it.
#[derive(Serialize, JsonSchema)]
pub(crate) struct TenantRecord {
	pub(crate) name: String,
	pub(crate) label: Option<String>,
	pub(crate) personal: bool,
	pub(crate) created_at: DateTime<Utc>,
	/// `identity` for a Tenant Creator, `deployment_administrator` for a Deployment Administrator.
	pub(crate) created_by_kind: String,
	/// The Tenant's current deletion; absent while the Tenant is in use.
	pub(crate) deletion: Option<TenantDeletionView>,
	/// Whether a Deployment Administrator may restore the Tenant now, decided
	/// by the database clock rather than the reader's.
	pub(crate) restorable: bool,
}

#[derive(Deserialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct TenantDeletionRequest {
	/// The Tenant name, typed by the person deleting the Tenant.
	#[validate(length(min = 1, max = 256))]
	pub(crate) confirmation: String,
}

/// A recorded Tenant Deletion.
#[derive(Serialize, JsonSchema)]
pub(crate) struct TenantDeletionView {
	pub(crate) tenant: String,
	pub(crate) deleted_at: DateTime<Utc>,
	/// `tenant_administrator` or `deployment_administrator`.
	pub(crate) deleted_by_kind: String,
	/// The end of the grace period: restoring is possible until then.
	pub(crate) purge_after: DateTime<Utc>,
	/// When the Tenant's data was purged; it can no longer be restored.
	pub(crate) purged_at: Option<DateTime<Utc>>,
}

/// A Tenant this External Identity is mapped into and that is deleted, as
/// the Tenant switcher shows it.
#[derive(Serialize, JsonSchema)]
pub(crate) struct DeletedTenantView {
	pub(crate) tenant: String,
	pub(crate) label: Option<String>,
	/// The Mapping that restores it, sent as the context selector.
	pub(crate) mapping_id: Uuid,
	pub(crate) personal: bool,
	pub(crate) purge_after: DateTime<Utc>,
	/// Whether this Mapping's subject may restore the Tenant now.
	pub(crate) restorable: bool,
}

#[derive(Deserialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct LabelUpdate {
	#[validate(length(min = 1, max = 256))]
	pub(crate) label: String,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct TenantLabel {
	pub(crate) name: String,
	pub(crate) label: String,
}
