use serde::{Deserialize, Serialize};
// Serializable contracts for workbench.

use crate::apps::registry::workbench::models::AgentDraft;
use crate::{knowledge::ReferenceDocument, registry::Entry};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, JsonSchema)]
pub struct Draft {
	pub id: Uuid,
	pub tenant: String,
	pub owner: String,
	pub revision: i64,
	pub entry: Value,
	pub documents: Value,
	pub release_notes: String,
	pub source_id: Option<String>,
	pub source_version: Option<String>,
	pub archived: bool,
	pub updated_at: DateTime<Utc>,
}

impl From<AgentDraft> for Draft {
	fn from(row: AgentDraft) -> Self {
		Self {
			id: row.id,
			tenant: row.tenant,
			owner: row.owner,
			revision: row.revision,
			entry: row.entry.into_inner(),
			documents: row.documents.into_inner(),
			release_notes: row.release_notes,
			source_id: row.source_id,
			source_version: row.source_version,
			archived: row.archived,
			updated_at: row.updated_at,
		}
	}
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateDraft {
	/// Operators must specify both fields. Authenticated subjects use their own identity.
	pub tenant: Option<String>,
	pub owner: Option<String>,
	pub entry: Entry,
	#[serde(default)]
	pub documents: Vec<ReferenceDocument>,
	#[serde(default)]
	pub release_notes: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveDraft {
	pub expected_revision: i64,
	pub entry: Entry,
	#[serde(default)]
	pub documents: Vec<ReferenceDocument>,
	#[serde(default)]
	pub release_notes: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevisionInput {
	pub expected_revision: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareInput {
	pub subject: String,
	pub can_edit: bool,
	pub enabled: bool,
	/// A shared draft always includes its private attachments.
	pub include_documents: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DraftShare {
	pub subject: String,
	pub can_edit: bool,
	pub documents_current: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferInput {
	pub expected_revision: i64,
	pub new_owner: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchiveInput {
	pub expected_revision: i64,
	pub archived: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdoptInput {
	pub tenant: String,
	pub owner: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Validation {
	pub draft_id: Uuid,
	pub revision: i64,
	pub valid: bool,
	pub message: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "WorkbenchContractsRegistration")]
pub struct Registration {
	pub draft_id: Uuid,
	pub revision: i64,
	pub entry: Entry,
	pub behavioral_tested: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RegisteredVersion {
	pub entry: Entry,
	/// Digest of the currently saved draft documents, independent of Registry state.
	pub draft_knowledge_digest: Option<String>,
	pub draft_revision: Option<i64>,
	pub registered_by: Option<String>,
	pub registered_at: Option<DateTime<Utc>>,
	pub release_notes: String,
	pub source_id: Option<String>,
	pub source_version: Option<String>,
	pub behavioral_tested: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub(crate) struct DraftPage {
	pub(crate) before_updated_at: Option<DateTime<Utc>>,
	pub(crate) before_id: Option<Uuid>,
}

use uuid::Uuid;
