use serde::{Deserialize, Serialize};
// Serializable contracts for authorization.

use crate::apps::federation::peer::models::{
	AuthorizationPeerMapping, AuthorizationPeerMappingHistory,
};
use crate::registry::Search;
use chrono::{DateTime, Utc};
use reinhardt::Validate;
use schemars::JsonSchema;

#[derive(Clone, Debug, PartialEq, Serialize, sqlx::FromRow, JsonSchema)]
pub struct PeerMapping {
	pub source_node: String,
	pub source_tenant: String,
	pub source_subject: String,
	pub tenant: String,
	pub credential_id: Uuid,
	pub enabled: bool,
	pub revision: i64,
	pub actor: String,
	pub updated_at: DateTime<Utc>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PeerMappingInput {
	pub source_node: String,
	pub source_tenant: String,
	pub source_subject: String,
	pub credential_id: Uuid,
	pub enabled: bool,
	pub expected_revision: i64,
}

#[derive(Deserialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct HistoryPage {
	#[serde(default)]
	#[validate(range(min = 0))]
	pub(crate) after: i64,
	#[serde(default = "page_size")]
	#[validate(range(min = 1, max = 200))]
	pub(crate) limit: i64,
}

#[derive(Serialize, JsonSchema)]
#[schemars(rename = "AuthorizationPeerMappingRevision")]
pub(crate) struct MappingRevision {
	pub(crate) sequence: i64,
	#[serde(flatten)]
	pub(crate) mapping: PeerMapping,
}

#[derive(Deserialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingPage {
	#[serde(default)]
	#[validate(range(min = 0))]
	pub(crate) offset: i64,
	#[serde(default = "page_size")]
	#[validate(range(min = 1, max = 200))]
	pub(crate) limit: i64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiscoveryInput {
	pub(crate) tenant: String,
	pub(crate) subject: String,
	pub(crate) search: Search,
}

pub(crate) fn page_size() -> i64 {
	100
}

impl From<AuthorizationPeerMapping> for PeerMapping {
	fn from(row: AuthorizationPeerMapping) -> Self {
		let tenant = row.tenant_record_id();
		let credential_id = row.credential_id();
		Self {
			source_node: row.source_node,
			source_tenant: row.source_tenant,
			source_subject: row.source_subject,
			tenant,
			credential_id,
			enabled: row.enabled,
			revision: row.revision,
			actor: row.actor,
			updated_at: row.updated_at,
		}
	}
}

impl From<AuthorizationPeerMappingHistory> for MappingRevision {
	fn from(row: AuthorizationPeerMappingHistory) -> Self {
		let tenant = row.tenant_record_id();
		let credential_id = row.credential_id();
		Self {
			sequence: row.sequence,
			mapping: PeerMapping {
				source_node: row.source_node,
				source_tenant: row.source_tenant,
				source_subject: row.source_subject,
				tenant,
				credential_id,
				enabled: row.enabled,
				revision: row.revision,
				actor: row.actor,
				updated_at: row.updated_at,
			},
		}
	}
}

use uuid::Uuid;
