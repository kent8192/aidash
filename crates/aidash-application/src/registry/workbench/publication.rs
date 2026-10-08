//! Advisory validation and immutable registration share current draft authority.
use super::{authorize, validate_content};
use crate::{
	Error, Result,
	ports::registry::workbench::{DraftRepository, DraftScope, PublicationScope},
	registry::{self, DefinitionValidation},
};
use aidash_domain::{
	identity::Principal,
	registry::{
		Entry, ReferenceDocument,
		knowledge::digest,
		workbench::{Draft, RegisteredVersion, Registration, RevisionInput, Validation},
	},
};
use serde_json::json;
use uuid::Uuid;
pub async fn behavioral_evidence(
	scope: &mut impl PublicationScope,
	draft: &Draft,
	entry: &Entry,
) -> Result<bool> {
	if let Some(previous) = scope.registered_evidence(entry).await? {
		if previous.draft_id != draft.id || previous.revision != draft.revision {
			return Err(Error::Conflict("version is already registered from another draft revision; choose a new semantic version".into()));
		}
		return Ok(previous.behavioral_tested);
	}
	scope.completed_test(draft).await
}

pub async fn validate<R: DraftRepository>(
	repository: &R,
	validation: &DefinitionValidation,
	id: Uuid,
	input: RevisionInput,
) -> Result<Validation> {
	let mut scope = repository.begin().await?;
	let draft = scope.read(id, false).await?;
	authorize(&mut scope, &draft, "agent_draft.write", true).await?;
	if input.expected_revision != draft.revision {
		return Err(Error::Conflict("draft revision changed".into()));
	}
	// Validation is advisory; Register repeats all checks on the locked revision.
	let result = validate_content(&mut scope, validation, &draft, repository.node_id()).await;
	scope.commit().await?;
	Ok(Validation {
		draft_id: id,
		revision: draft.revision,
		valid: result.is_ok(),
		message: result
			.err()
			.map_or_else(|| "Technical validation passed".into(), |e| e.to_string()),
	})
}

pub async fn versions<R: DraftRepository>(
	repository: &R,
	id: Uuid,
) -> Result<Vec<RegisteredVersion>>
where
	R::Scope: PublicationScope,
{
	let mut scope = repository.begin().await?;
	let draft = scope.read(id, false).await?;
	authorize(&mut scope, &draft, "agent_draft.read", true).await?;
	let managed_id = draft.entry["id"]
		.as_str()
		.ok_or_else(|| Error::Invalid("draft has no managed identity".into()))?;

	let rows = scope.registrations(id, managed_id).await?;
	let draft_knowledge_digest = draft
		.documents
		.as_array()
		.filter(|docs| !docs.is_empty())
		.map(|_| digest(&draft.documents));
	let mut versions = Vec::new();
	for row in rows {
		let entry = registry::effective(&mut scope, managed_id, &row.version).await?;
		let registered_knowledge_digest =
			private_digest(&mut scope, &entry, repository.node_id()).await?;
		versions.push(RegisteredVersion {
			registered_knowledge_digest,
			draft_knowledge_digest: draft_knowledge_digest.clone(),
			entry,
			draft_revision: Some(row.revision),
			registered_by: Some(row.actor),
			registered_at: Some(row.registered_at),
			release_notes: row.release_notes,
			source_id: row.source_id,
			source_version: row.source_version,
			behavioral_tested: Some(row.behavioral_tested),
		});
	}
	if draft.source_id.as_deref() == Some(managed_id)
		&& let Some(source_version) = &draft.source_version
		&& !versions
			.iter()
			.any(|item| &item.entry.version == source_version)
	{
		let entry = registry::effective(&mut scope, managed_id, source_version).await?;
		let registered_knowledge_digest =
			private_digest(&mut scope, &entry, repository.node_id()).await?;
		versions.push(RegisteredVersion {
			registered_knowledge_digest,
			draft_knowledge_digest: draft_knowledge_digest.clone(),
			entry,
			draft_revision: None,
			registered_by: None,
			registered_at: None,
			release_notes: String::new(),
			source_id: None,
			source_version: None,
			behavioral_tested: None,
		});
	}
	scope.commit().await?;
	Ok(versions)
}

pub async fn register<R: DraftRepository>(
	repository: &R,
	validation: &DefinitionValidation,
	id: Uuid,
	input: RevisionInput,
) -> Result<Registration>
where
	R::Scope: PublicationScope,
{
	let actor = repository.principal();
	let mut scope = repository.begin().await?;
	let draft = scope.read(id, true).await?;
	authorize(&mut scope, &draft, "agent_draft.register", true).await?;
	if draft.archived || input.expected_revision != draft.revision {
		return Err(Error::Conflict(
			"draft revision changed or is archived".into(),
		));
	}
	let entry = validate_content(&mut scope, validation, &draft, repository.node_id()).await?;

	let behavioral_tested = behavioral_evidence(&mut scope, &draft, &entry).await?;
	let documents: Vec<ReferenceDocument> = serde_json::from_value(draft.documents.clone())?;
	if !documents.is_empty() {
		let mut raw: Entry = serde_json::from_value(draft.entry.clone())?;
		registry::bindings::private::detach(
			&mut scope,
			&mut raw,
			repository.node_id(),
			draft
				.source_id
				.as_deref()
				.zip(draft.source_version.as_deref()),
		)
		.await?;
		let source =
			registry::bindings::private::attach(&mut raw, repository.node_id(), &draft.documents)?;
		registry::register_definition(&mut scope, validation, &source, repository.node_id())
			.await?;
		scope
			.insert_documents(&source, draft.documents.clone())
			.await?;
	}
	let inserted =
		registry::register_definition(&mut scope, validation, &entry, repository.node_id()).await?;
	let registered_by = match &actor {
		Principal::Operator => "operator",
		Principal::Subject { subject, .. } => subject,
	};
	scope
		.record_registration(&draft, &entry, registered_by, behavioral_tested)
		.await?;
	if inserted {
		scope.append_event("registry.registered", json!({"id":entry.id,"version":entry.version,"kind":"agent","draft_id":id,"draft_revision":draft.revision})).await?;
	}
	scope.commit().await?;
	Ok(Registration {
		draft_id: id,
		revision: draft.revision,
		entry,
		behavioral_tested,
	})
}

async fn private_digest(
	scope: &mut impl crate::ports::registry::DefinitionLookup,
	entry: &Entry,
	node: &str,
) -> Result<Option<String>> {
	let config: aidash_domain::registry::bindings::AgentBindings =
		serde_json::from_value(entry.config.clone())?;
	for binding in config.bindings.iter().filter(|b| {
		b.kind == aidash_domain::registry::bindings::BindingKind::Source
			&& b.target.registry_node == node
	}) {
		let source = scope
			.definition(&binding.target.id, &binding.target.version)
			.await?;
		let Some(context) =
			aidash_domain::registry::bindings::sources::validate_definition(&source)?
		else {
			continue;
		};
		if let aidash_domain::registry::bindings::sources::NativeSource::PrivateReferences {
			digest,
		} = context.source
		{
			return Ok(Some(digest));
		}
	}
	Ok(None)
}
