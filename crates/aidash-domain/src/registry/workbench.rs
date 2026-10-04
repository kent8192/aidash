//! Creator draft state and content rules are independent of persistence.
use crate::{
	Error, Result,
	registry::{
		AgentConfig, Entry, ReferenceDocument,
		knowledge::{digest, validate as validate_documents},
	},
};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
pub struct DraftPage {
	pub before_updated_at: Option<DateTime<Utc>>,
	pub before_id: Option<Uuid>,
}
pub fn check_content(
	entry: &Entry,
	documents: &[ReferenceDocument],
	release_notes: &str,
) -> Result<()> {
	if entry.kind != "agent" || entry.id.is_empty() || entry.id.len() > 100 {
		return Err(Error::Invalid(
			"draft must contain a managed agent identity".into(),
		));
	}
	if !documents.is_empty() {
		validate_documents(documents)?;
	}
	if release_notes.len() > 8192 {
		return Err(Error::Invalid("release notes exceed 8 KiB".into()));
	}
	let _: AgentConfig =
		serde_json::from_value(entry.config.clone()).map_err(|e| Error::Invalid(e.to_string()))?;
	Ok(())
}
pub fn new_draft_defaults(entry: &mut Entry) -> Result<()> {
	let config = entry
		.config
		.as_object_mut()
		.ok_or_else(|| Error::Invalid("agent config must be an object".into()))?;
	for key in [
		"allow_task_creation",
		"allow_task_delegation",
		"allow_memory_write",
		"allow_workspace_retrieval",
		"allow_cross_conversation_memory",
	] {
		match config.get(key) {
			Some(Value::Bool(_)) => {}
			None => {
				config.insert(key.into(), json!(false));
			}
			Some(_) => return Err(Error::Invalid(format!("{key} must be a boolean"))),
		}
	}
	Ok(())
}
/// A share remains usable only for the exact acknowledged private document set.
pub fn current_share(documents: &Value, share: Option<(bool, String)>) -> Option<(bool, String)> {
	share.filter(|(_, documents_digest)| documents_digest == &digest(documents))
}
#[cfg(test)]
mod tests;
