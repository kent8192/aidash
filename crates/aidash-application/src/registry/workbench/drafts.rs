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

use aidash_domain::{identity::Principal, registry::workbench::DraftPage};
pub async fn list<R: DraftRepository>(repository: &R, page: DraftPage) -> Result<Vec<Draft>> {
	let actor = repository.principal();
	let mut scope = repository.begin().await?;
	let mut visible = Vec::new();
	let mut cursor = match (page.before_updated_at, page.before_id) {
		(None, None) => None,
		(Some(updated_at), Some(id)) => Some((updated_at, id)),
		_ => {
			return Err(Error::Invalid(
				"draft cursor requires both timestamp and ID".into(),
			));
		}
	};
	let tenant = match &actor {
		Principal::Operator => None,
		Principal::Subject { tenant, .. } => Some(tenant),
	};
	loop {
		let rows = scope.page(tenant.map(String::as_str), cursor).await?;
		let more = rows.len() == 100;
		cursor = rows.last().map(|row| (row.updated_at, row.id));
		for row in rows {
			match authorize(&mut scope, &row, "agent_draft.read", true).await {
				Ok(()) => visible.push(row),
				Err(Error::Forbidden) => {}
				Err(error) => return Err(error),
			}
			if visible.len() == 100 {
				break;
			}
		}
		if visible.len() == 100 || !more {
			break;
		}
	}
	scope.commit().await?;
	Ok(visible)
}
pub async fn get<R: DraftRepository>(repository: &R, id: Uuid) -> Result<Draft> {
	let mut scope = repository.begin().await?;
	let draft = scope.read(id, false).await?;
	authorize(&mut scope, &draft, "agent_draft.read", true).await?;
	scope.commit().await?;
	Ok(draft)
}

use aidash_domain::registry::{
	Entry,
	workbench::{AdoptInput, RevisionInput},
};
use serde_json::json;
pub async fn duplicate<R: DraftRepository>(
	repository: &R,
	id: Uuid,
	input: RevisionInput,
) -> Result<Draft> {
	let actor = repository.principal();
	let mut scope = repository.begin().await?;
	let original = scope.read(id, true).await?;
	authorize(&mut scope, &original, "agent_draft.read", true).await?;
	if original.revision != input.expected_revision {
		return Err(Error::Conflict("draft revision changed".into()));
	}
	let id = Uuid::now_v7();
	let mut entry: Entry = serde_json::from_value(original.entry.clone())?;
	let source_id = entry.id.clone();
	let source_version = entry.version.clone();
	entry.id = id.to_string();
	entry.version = "1.0.0".into();
	new_draft_defaults(&mut entry)?;
	let prospective = Draft {
		id,
		tenant: original.tenant.clone(),
		owner: match &actor {
			Principal::Operator => original.owner.clone(),
			Principal::Subject { subject, .. } => subject.clone(),
		},
		revision: 1,
		entry: serde_json::to_value(&entry)?,
		documents: original.documents.clone(),
		release_notes: String::new(),
		source_id: Some(source_id),
		source_version: Some(source_version),
		archived: false,
		updated_at: Utc::now(),
	};
	authorize(&mut scope, &prospective, "agent_draft.create", false).await?;
	let copied = scope.insert(&prospective, &entry.id).await?;
	scope.append_event("agent_draft.duplicated", json!({"draft_id":id,"source_id":prospective.source_id,"source_version":prospective.source_version,"tenant":prospective.tenant})).await?;
	scope.commit().await?;
	Ok(copied)
}
pub async fn adopt<R: DraftRepository>(
	repository: &R,
	(id, version): (String, String),
	input: AdoptInput,
) -> Result<Draft> {
	let actor = repository.principal();
	if !matches!(actor, Principal::Operator) {
		return Err(Error::Forbidden);
	}
	let (tenant, owner) = author_identity(&actor, Some(&input.tenant), Some(&input.owner))?;
	let mut entry = repository.original_entry(&id, &version).await?;
	if entry.kind != "agent" {
		return Err(Error::Invalid(
			"only agents can be assigned to Creator".into(),
		));
	}

	let documents = repository.original_documents(&entry).await?;
	new_draft_defaults(&mut entry)?;
	let mut scope = repository.begin().await?;
	target_enabled(&mut scope, &tenant, &owner).await?;
	let existing = scope.managed(&id).await?;
	if existing {
		return Err(Error::Conflict("agent identity is already managed".into()));
	}
	let draft_id = Uuid::now_v7();
	let prospective = Draft {
		id: draft_id,
		tenant: tenant.clone(),
		owner: owner.clone(),
		revision: 1,
		entry: serde_json::to_value(&entry)?,
		documents,
		release_notes: String::new(),
		source_id: Some(id.clone()),
		source_version: Some(version.clone()),
		archived: false,
		updated_at: Utc::now(),
	};
	let draft = scope.insert(&prospective, &id).await?;
	scope
		.append_event(
			"agent_draft.adopted",
			json!({"draft_id":draft_id,"agent_id":id,"version":version,"tenant":tenant,"owner":owner}),
		)
		.await?;
	scope.commit().await?;
	Ok(draft)
}

use super::owner_only;
use aidash_domain::registry::{
	knowledge::digest,
	workbench::{DraftShare, ShareInput},
};
pub async fn share<R: DraftRepository>(
	repository: &R,
	id: Uuid,
	input: ShareInput,
) -> Result<Draft> {
	let actor = repository.principal();
	let mut scope = repository.begin().await?;
	let draft = scope.read(id, true).await?;
	owner_only(&actor, &draft)?;
	authorize(&mut scope, &draft, "agent_draft.share", false).await?;
	if draft.owner == input.subject {
		return Err(Error::Invalid("owner does not need a share".into()));
	}
	if input.enabled && draft.documents != json!([]) && !input.include_documents {
		return Err(Error::Invalid(
			"sharing this draft also shares its private documents; acknowledge include_documents"
				.into(),
		));
	}
	if input.enabled {
		target_enabled(&mut scope, &draft.tenant, &input.subject).await?;
	}
	if input.enabled {
		scope
			.save_share(
				id,
				&input.subject,
				input.can_edit,
				&digest(&draft.documents),
			)
			.await?;
	} else {
		scope.remove_share(id, &input.subject).await?;
	}
	scope.append_event("agent_draft.share_changed", json!({"draft_id":id,"tenant":draft.tenant,"subject":input.subject,"enabled":input.enabled,"can_edit":input.can_edit})).await?;
	scope.commit().await?;
	Ok(draft)
}
pub async fn shares<R: DraftRepository>(repository: &R, id: Uuid) -> Result<Vec<DraftShare>> {
	let actor = repository.principal();
	let mut scope = repository.begin().await?;
	let draft = scope.read(id, false).await?;
	owner_only(&actor, &draft)?;
	authorize(&mut scope, &draft, "agent_draft.share", false).await?;
	let rows = scope.shares(id).await?;
	scope.commit().await?;
	let current_digest = digest(&draft.documents);
	Ok(rows
		.into_iter()
		.map(|row| DraftShare {
			subject: row.subject,
			can_edit: row.can_edit,
			documents_current: row.documents_digest == current_digest,
		})
		.collect())
}

#[cfg(test)]
mod tests;
