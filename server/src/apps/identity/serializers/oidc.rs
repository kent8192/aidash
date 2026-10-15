use serde::{Deserialize, Serialize};
// Serializable contracts for authorization.

use chrono::{DateTime, Utc};
use reinhardt::Validate;
use schemars::JsonSchema;

#[derive(Serialize, JsonSchema)]
pub(crate) struct Configuration {
	pub(crate) enabled: bool,
	pub(crate) desktop_protocol: u8,
	pub(crate) provider: &'static str,
	pub(crate) login_url: Option<&'static str>,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct LoginQuery {
	pub(crate) return_to: Option<String>,
	pub(crate) org: Option<String>,
}

/// Email-domain discovery input. The address is only parsed for its domain and
/// is never stored or logged.
#[derive(Deserialize, JsonSchema)]
pub(crate) struct LoginDiscovery {
	#[serde(default)]
	pub(crate) email: String,
	pub(crate) return_to: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct CallbackQuery {
	pub(crate) state: String,
	pub(crate) code: String,
}

#[derive(Serialize, JsonSchema)]
pub struct MappingView {
	pub(crate) id: Uuid,
	pub(crate) tenant: String,
	/// The Tenant label, absent only for a Tenant without one.
	pub(crate) label: Option<String>,
	pub(crate) subject: String,
	pub(crate) personal: bool,
	/// False when the mapping's non-exportable credential is revoked or expired,
	/// so selecting it would fail authentication.
	pub(crate) credential_active: bool,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct SessionView {
	pub(crate) id: Uuid,
	pub(crate) operator: bool,
	pub(crate) mappings: Vec<MappingView>,
	/// True when Aidash made a Personal Tenant for this identity, which may
	/// request shared Tenant access whether or not its Personal Tenant Mapping
	/// is still enabled.
	pub(crate) personal_account: bool,
	/// Whether the Tenancy Rules let this identity create Tenants now.
	pub(crate) tenant_creation: bool,
	/// Whether this Deployment issues Invitations.
	pub(crate) invitations: bool,
}

#[derive(Serialize, JsonSchema)]
#[schemars(rename = "AuthorizationOidcRegistration")]
pub(crate) struct Registration {
	pub(crate) id: Uuid,
	pub(crate) identity_id: Uuid,
	pub(crate) status: String,
	pub(crate) created_at: DateTime<Utc>,
	pub(crate) expires_at: DateTime<Utc>,
	pub(crate) decided_at: Option<DateTime<Utc>>,
	/// The Tenant the request is addressed to; absent when only Deployment
	/// Administrators may decide it.
	pub(crate) tenant: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct IdentityView {
	pub(crate) id: Uuid,
	pub(crate) issuer: String,
	pub(crate) subject: String,
	pub(crate) gcip_tenant: Option<String>,
	pub(crate) verified_email: Option<String>,
	pub(crate) display_name: Option<String>,
	pub(crate) disabled_at: Option<DateTime<Utc>>,
	/// Set by Display Erasure; the erased values are never retained.
	pub(crate) display_erased_at: Option<DateTime<Utc>>,
	pub(crate) display_erased_by: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct AdminIdentityPage {
	#[serde(default)]
	pub(crate) offset: u64,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct AdminMapping {
	pub(crate) id: Uuid,
	pub(crate) identity_id: Uuid,
	pub(crate) tenant: String,
	pub(crate) subject: String,
	pub(crate) enabled: bool,
	pub(crate) revision: i64,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct AdminMappingPage {
	#[serde(default)]
	pub(crate) offset: u64,
}

#[derive(Deserialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct Approval {
	#[validate(length(min = 1, max = 256))]
	pub(crate) tenant: String,
	#[validate(length(min = 1, max = 256))]
	pub(crate) subject: String,
	/// A Deployment Administrator approving into a subject that does not exist
	/// yet creates it as a user subject in these groups, which may be any of
	/// the policy's groups.
	#[serde(default)]
	pub(crate) groups: Option<std::collections::BTreeSet<String>>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct ApprovedMapping {
	pub(crate) id: Uuid,
	pub(crate) identity_id: Uuid,
	pub(crate) tenant: String,
	pub(crate) subject: String,
}

#[derive(Deserialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorGrantInput {
	pub(crate) enabled: bool,
	#[validate(range(min = 0))]
	pub(crate) expected_revision: i64,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct AdminOperatorGrant {
	pub(crate) identity_id: Uuid,
	pub(crate) enabled: bool,
	pub(crate) revision: i64,
}

#[derive(Deserialize, JsonSchema, Validate)]
#[schemars(rename = "AuthorizationOidcMappingRevision")]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingRevision {
	#[validate(range(min = 1))]
	pub(crate) expected_revision: i64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BackchannelLogout {
	pub(crate) logout_token: String,
}

use uuid::Uuid;
