//! Contracts of the Tenant Administrator endpoints under `/api/tenants/{tenant}`.
use chrono::{DateTime, Utc};
use reinhardt::Validate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

/// The actions the caller's Tenant policy permits, and what it may assign.
#[derive(Serialize, JsonSchema)]
pub(crate) struct TenantAdministration {
	pub(crate) tenant: String,
	pub(crate) actions: Vec<String>,
	/// Empty unless the caller holds a Tenant Administrator action.
	pub(crate) assignable_groups: BTreeSet<String>,
	/// The policy revision a membership update must name.
	pub(crate) policy_revision: Option<i64>,
}

/// Display Attributes recognize the person; they never decide authority.
#[derive(Serialize, JsonSchema)]
pub(crate) struct TenantIdentity {
	pub(crate) id: Uuid,
	pub(crate) verified_email: Option<String>,
	pub(crate) display_name: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct TenantRegistration {
	pub(crate) id: Uuid,
	pub(crate) identity: TenantIdentity,
	pub(crate) created_at: DateTime<Utc>,
	pub(crate) expires_at: DateTime<Utc>,
}

#[derive(Deserialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct TenantApproval {
	#[validate(length(min = 1, max = 256))]
	pub(crate) subject: String,
	/// Assignable Groups for a subject created by this approval. A re-approval
	/// keeps the existing subject's groups and must send none.
	#[serde(default)]
	pub(crate) groups: BTreeSet<String>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct TenantMapping {
	pub(crate) id: Uuid,
	pub(crate) identity: TenantIdentity,
	pub(crate) subject: String,
	pub(crate) enabled: bool,
	pub(crate) revision: i64,
	/// The subject's Assignable Group memberships.
	pub(crate) groups: BTreeSet<String>,
}

#[derive(Deserialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct MembershipUpdate {
	/// The complete set of Assignable Groups the subject should belong to.
	pub(crate) groups: BTreeSet<String>,
	#[validate(range(min = 1))]
	pub(crate) expected_policy_revision: i64,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct Membership {
	pub(crate) subject: String,
	pub(crate) groups: BTreeSet<String>,
	pub(crate) policy_revision: i64,
}
