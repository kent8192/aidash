use serde::{Deserialize, Serialize};
// Serializable contracts for authorization.

use chrono::{DateTime, Utc};
use reinhardt::Validate;
use schemars::JsonSchema;

#[derive(Serialize, JsonSchema)]
pub(crate) struct Configuration {
	pub(crate) enabled: bool,
	pub(crate) provider: &'static str,
	pub(crate) login_url: Option<&'static str>,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct LoginQuery {
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
	pub(crate) subject: String,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct SessionView {
	pub(crate) id: Uuid,
	pub(crate) operator: bool,
	pub(crate) mappings: Vec<MappingView>,
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
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct IdentityView {
	pub(crate) id: Uuid,
	pub(crate) issuer: String,
	pub(crate) subject: String,
	pub(crate) disabled_at: Option<DateTime<Utc>>,
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
