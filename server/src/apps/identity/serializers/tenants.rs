//! Contracts of the Tenant creation API and Tenant labels.
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
	/// `identity` for a Tenant Creator, `operator` for a Deployment Administrator.
	pub(crate) created_by_kind: String,
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
