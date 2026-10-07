//! Request and response contracts.
use crate::apps::identity::policy::PolicyBundle;
use crate::registry::EntityRef;
use schemars::JsonSchema;
use serde::Deserialize;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogInput {
	pub(crate) entry: EntityRef,
	pub(crate) expected_revision: i64,
	pub(crate) enabled: bool,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CredentialInput {
	pub(crate) subject: String,
	#[serde(default = "credential_lifetime")]
	pub(crate) expires_in_seconds: i64,
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
	#[serde(skip_serializing_if = "Option::is_none")]
	pub(crate) pending_host_packages:
		Option<aidash_application::marketplace::operations::host_packages::PendingHostPackages>,
	#[serde(flatten)]
	pub(crate) snapshot: crate::apps::identity::serializers::contracts::Snapshot,
	pub(crate) pending_transactions: Vec<uuid::Uuid>,
}
