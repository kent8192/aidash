//! Request and response contracts.
use crate::apps::identity::policy::PolicyBundle;
use crate::registry::EntityRef;
use schemars::JsonSchema;
use serde::Deserialize;
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
}
/// The Node whose Catalog Approvals are read.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogQuery {
	pub(crate) node: aidash_domain::nodes::NodeName,
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
