//! Managed draft creation and editing preserve the current locked revision.
use super::{author_identity, authorize, target_enabled};
use crate::{
	Error, Result,
	ports::registry::workbench::{DraftRepository, DraftScope},
};
use aidash_domain::registry::workbench::{
	CreateDraft, Draft, SaveDraft, check_content, new_draft_defaults,
};
use chrono::Utc;
use uuid::Uuid;

pub async fn create<R: DraftRepository>(repository: &R, mut input: CreateDraft) -> Result<Draft> {
	let actor = repository.principal();
	let (tenant, owner) = author_identity(&actor, input.tenant.as_deref(), input.owner.as_deref())?;
	let id = Uuid::now_v7();
	if !input.entry.id.is_empty() {
		return Err(Error::Invalid("new draft must leave entry.id empty; use an authorized version flow for existing identities".into()));
	}
	input.entry.id = id.to_string();
	new_draft_defaults(&mut input.entry)?;
	check_content(&input.entry, &input.documents, &input.release_notes)?;
	let entry = serde_json::to_value(&input.entry)?;
	let documents = serde_json::to_value(&input.documents)?;
	let mut scope = repository.begin().await?;
	target_enabled(&mut scope, &tenant, &owner).await?;
	let prospective = Draft {
		id,
		tenant,
		owner,
		revision: 1,
		entry: entry.clone(),
		documents: documents.clone(),
		release_notes: input.release_notes.clone(),
		source_id: None,
		source_version: None,
		archived: false,
		updated_at: Utc::now(),
	};
	authorize(&mut scope, &prospective, "agent_draft.create", false).await?;
	let saved = scope.insert(&prospective, &input.entry.id).await?;
	scope.commit().await?;
	Ok(saved)
}

pub async fn save<R: DraftRepository>(
	repository: &R,
	id: Uuid,
	mut input: SaveDraft,
) -> Result<Draft> {
	let mut scope = repository.begin().await?;
	let draft = scope.read(id, true).await?;
	authorize(&mut scope, &draft, "agent_draft.write", true).await?;
	if draft.revision != input.expected_revision {
		return Err(Error::Conflict(
			"draft revision changed; local edits were not saved".into(),
		));
	}
	if draft.archived {
		return Err(Error::Conflict(
			"restore the archived draft before editing".into(),
		));
	}
	if input.entry.id != draft.entry["id"].as_str().unwrap_or_default() {
		return Err(Error::Invalid(
			"managed agent identity cannot change".into(),
		));
	}
	new_draft_defaults(&mut input.entry)?;
	check_content(&input.entry, &input.documents, &input.release_notes)?;
	let entry = serde_json::to_value(&input.entry)?;
	let documents = serde_json::to_value(&input.documents)?;
	let saved = scope
		.save_content(id, entry, documents, &input.release_notes)
		.await?;
	scope.commit().await?;
	Ok(saved)
}

#[cfg(test)]
mod tests;
