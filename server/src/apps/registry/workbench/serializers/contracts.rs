use serde::{Deserialize, Serialize};
// Serializable contracts for workbench.

use crate::apps::registry::workbench::models::AgentDraft;
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

use uuid::Uuid;

pub(crate) use aidash_domain::registry::workbench::DraftPage;
pub use aidash_domain::registry::workbench::{
	AdoptInput, ArchiveInput, CreateDraft, DraftShare, RegisteredVersion, Registration,
	RevisionInput, SaveDraft, ShareInput, TransferInput, Validation,
};

impl From<Draft> for aidash_domain::registry::workbench::Draft {
	fn from(row: Draft) -> Self {
		Self {
			id: row.id,
			tenant: row.tenant,
			owner: row.owner,
			revision: row.revision,
			entry: row.entry,
			documents: row.documents,
			release_notes: row.release_notes,
			source_id: row.source_id,
			source_version: row.source_version,
			archived: row.archived,
			updated_at: row.updated_at,
		}
	}
}

impl From<aidash_domain::registry::workbench::Draft> for Draft {
	fn from(row: aidash_domain::registry::workbench::Draft) -> Self {
		Self {
			id: row.id,
			tenant: row.tenant,
			owner: row.owner,
			revision: row.revision,
			entry: row.entry,
			documents: row.documents,
			release_notes: row.release_notes,
			source_id: row.source_id,
			source_version: row.source_version,
			archived: row.archived,
			updated_at: row.updated_at,
		}
	}
}
