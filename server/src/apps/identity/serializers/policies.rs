//! Request and response contracts.
use crate::apps::identity::policy::PolicyBundle;
use crate::registry::EntityRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogInput {
	/// The Node the approval applies to.
	pub(crate) node: aidash_domain::nodes::NodeName,
	pub(crate) entry: EntityRef,
	pub(crate) expected_revision: i64,
	pub(crate) enabled: bool,
	/// Assignable Groups for the subject of an approved agent version (ADR 0062).
	#[serde(default)]
	pub(crate) groups: Option<std::collections::BTreeSet<String>>,
	/// Subjects and Assignable Groups that may use the version on this Node
	/// (ADR 0095). Absent keeps an existing restriction.
	#[serde(default)]
	pub(crate) restriction: Option<aidash_domain::identity::catalog::CatalogRestriction>,
}
/// The Node whose Catalog Approvals are read.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogQuery {
	pub(crate) node: aidash_domain::nodes::NodeName,
}

/// One root definition on one Node, named as `kind/id@version` or `id@version`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogReviewQuery {
	pub(crate) node: aidash_domain::nodes::NodeName,
	pub(crate) entry: String,
}

/// What one review submission wrote, in the caller's transaction.
#[derive(Serialize, JsonSchema)]
pub(crate) struct CatalogReviewResult {
	pub(crate) bindings: Vec<crate::apps::identity::serializers::catalog::Binding>,
	pub(crate) installations: Vec<aidash_domain::marketplace::Installation>,
}

/// One Tenant subject's Catalog Approval Request (ADR 0096).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogRequestInput {
	pub(crate) node: aidash_domain::nodes::NodeName,
	pub(crate) root: crate::registry::EntityRef,
	pub(crate) reason: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogRequestDecline {
	pub(crate) reason: String,
}
#[derive(Deserialize, JsonSchema)]
pub(crate) struct CatalogRequestPage {
	#[serde(default)]
	pub(crate) node: Option<aidash_domain::nodes::NodeName>,
	#[serde(default)]
	pub(crate) offset: u64,
	#[serde(default = "request_page_size")]
	pub(crate) limit: u64,
}
fn request_page_size() -> u64 {
	50
}
/// What a requester or an approver needs to know, without naming hidden items
/// or the subjects that hold approval authority.
#[derive(Serialize, JsonSchema)]
pub(crate) struct CatalogRequestReview {
	pub(crate) ready: bool,
	pub(crate) hidden: u64,
	pub(crate) total: u64,
	pub(crate) approvable: u64,
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CatalogRequestNextAction {
	WaitingForApprover,
	Ready,
	Declined,
	Withdrawn,
	Stale,
	Fulfilled,
}
#[derive(Serialize, JsonSchema)]
pub(crate) struct CatalogApprovalRequestView {
	pub(crate) id: uuid::Uuid,
	pub(crate) node: aidash_domain::nodes::NodeName,
	pub(crate) root: crate::registry::EntityRef,
	pub(crate) requester: String,
	pub(crate) reason: String,
	pub(crate) state: aidash_domain::identity::catalog_request::CatalogApprovalRequestState,
	pub(crate) decline_reason: Option<String>,
	pub(crate) closure_digest: String,
	pub(crate) created_at: chrono::DateTime<chrono::Utc>,
	pub(crate) updated_at: chrono::DateTime<chrono::Utc>,
	pub(crate) revision: i64,
	pub(crate) next_action: CatalogRequestNextAction,
	pub(crate) review: Option<CatalogRequestReview>,
}
#[derive(Serialize, JsonSchema)]
pub(crate) struct CatalogApprovalRequestPage {
	pub(crate) requests: Vec<CatalogApprovalRequestView>,
	pub(crate) next_offset: Option<u64>,
}
/// Approvals of one dependency closure, decided together.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogApprovals {
	/// The Node the approvals apply to.
	pub(crate) node: aidash_domain::nodes::NodeName,
	pub(crate) approvals: Vec<aidash_application::authorization::catalog::approvals::Approval>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CredentialInput {
	pub(crate) subject: String,
	#[serde(default = "credential_lifetime")]
	pub(crate) expires_in_seconds: i64,
}
/// A page of credentials; `mine` keeps only those the caller issued for
/// itself through its Mapping.
#[derive(Deserialize, JsonSchema)]
pub(crate) struct CredentialPage {
	#[serde(default)]
	pub(crate) offset: u64,
	#[serde(default)]
	pub(crate) mine: bool,
	/// Return only credentials that can still authenticate.
	#[serde(default)]
	pub(crate) active: bool,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuthorizationUpdate {
	pub(crate) expected_revision: i64,
	pub(crate) bundle: PolicyBundle,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuthorizationPage {
	#[serde(default)]
	pub(crate) after: i64,
	#[serde(default = "page_size")]
	pub(crate) limit: i64,
}
fn credential_lifetime() -> i64 {
	3600
}
fn page_size() -> i64 {
	100
}

#[derive(serde::Serialize, JsonSchema)]
pub(crate) struct CredentialRevocation {
	#[serde(flatten)]
	pub(crate) credential: crate::apps::identity::serializers::identity::Credential,
	pub(crate) pending_transactions: Vec<uuid::Uuid>,
}

#[derive(serde::Serialize, JsonSchema)]
pub(crate) struct PolicyReplacement {
	#[serde(flatten)]
	pub(crate) snapshot: crate::apps::identity::serializers::contracts::Snapshot,
	pub(crate) pending_transactions: Vec<uuid::Uuid>,
}
