//! Private registration keeps document admission and publication atomic.
use super::{DefinitionValidation, register_definition};
use crate::{
	Error, Result,
	ports::registry::{DefinitionLookup, PrivateKnowledgeRead, PrivateKnowledgeScope},
};
use aidash_domain::registry::{AgentConfig, Entry, ReferenceDocument, knowledge};
use serde_json::{Value, json};
use uuid::Uuid;

pub struct PersonalDraft {
	entry: Entry,
	documents: Vec<ReferenceDocument>,
}
pub struct PersonalRegistration {
	pub entry: Entry,
	pub documents: Value,
	pub source: Entry,
}

/// Shape validation precedes transport-level idempotency-key parsing.
pub fn validate_input(entry: Entry, documents: Vec<ReferenceDocument>) -> Result<PersonalDraft> {
	if entry.binding_normalization.is_some() {
		return Err(Error::Invalid("Binding normalization is read-only".into()));
	}
	if entry.kind != "agent" {
		return Err(Error::Invalid(
			"personal registration requires an agent".into(),
		));
	}
	let _: AgentConfig = serde_json::from_value(entry.config.clone())
		.map_err(|error| Error::Invalid(error.to_string()))?;
	knowledge::validate(&documents)?;
	Ok(PersonalDraft { entry, documents })
}

/// Prompt admission includes private text but the discoverable definition carries only a digest.
pub async fn prepare(
	scope: &mut dyn DefinitionLookup,
	validation: &DefinitionValidation,
	draft: PersonalDraft,
	node: &str,
) -> Result<PersonalRegistration> {
	let documents = serde_json::to_value(draft.documents)?;
	let mut entry = draft.entry;
	let source = super::bindings::private::attach(&mut entry, node, &documents)?;
	let context = json!({"reference_documents":documents.clone()});

	let mut preview = super::bindings::private::Preview {
		scope,
		source: &source,
	};
	let mut catalog = super::bindings::catalog::LookupCatalog {
		definitions: &mut preview,
		node,
	};
	// Admission below repeats resolution after reserving the real identity.
	let mut preview_entry = entry.clone();
	if preview_entry.id.is_empty() {
		preview_entry.id = "pending-personal-agent".into();
	}
	let snapshot = super::bindings::resolve(
		&mut catalog,
		validation,
		aidash_domain::registry::bindings::QualifiedRef {
			registry_node: node.into(),
			id: preview_entry.id.clone(),
			version: entry.version.clone(),
		},
		&preview_entry,
		false,
	)
	.await?;
	validation.bound_prompt_headroom(&snapshot, &context)?;
	entry.normalize_agent(node)?;
	Ok(PersonalRegistration {
		entry,
		documents,
		source,
	})
}

/// Reservation, protected references, definition, private text and outbox share one scope.
pub async fn register(
	scope: &mut dyn PrivateKnowledgeScope,
	validation: &DefinitionValidation,
	mut registration: PersonalRegistration,
	key: Uuid,
	node: &str,
) -> Result<Entry> {
	scope.assign_id(&mut registration.entry, Some(key)).await?;
	let preview_source = registration.source.id.clone();
	let mut input: aidash_domain::registry::bindings::AgentBindings =
		serde_json::from_value(registration.entry.config.clone())?;
	input.bindings.retain(|b| b.target.id != preview_source);
	registration.entry.config = serde_json::to_value(input)?;
	registration.source =
		super::bindings::private::attach(&mut registration.entry, node, &registration.documents)?;
	registration.entry.normalize_agent(node)?;
	register_definition(scope, validation, &registration.source, node).await?;
	let inserted = register_definition(scope, validation, &registration.entry, node).await?;
	scope
		.insert_documents(&registration.source, registration.documents)
		.await?;
	if inserted {
		scope
			.append_event(
				"registry.registered",
				json!({"id":registration.entry.id,"version":registration.entry.version,"kind":"agent"}),
			)
			.await?;
	}
	Ok(registration.entry)
}

/// Private context is available only on its owning node, with the original digest.
pub async fn load(scope: &mut dyn PrivateKnowledgeRead, entry: &Entry) -> Result<Value> {
	let Some(descriptor) = aidash_domain::registry::bindings::sources::validate_definition(entry)?
	else {
		return Ok(json!([]));
	};
	let aidash_domain::registry::bindings::sources::NativeSource::PrivateReferences {
		digest: expected,
	} = descriptor.source
	else {
		return Ok(json!([]));
	};
	let documents = scope.documents(entry).await?.ok_or_else(|| Error::Invalid("private documents are unavailable on this node; run the original agent on its owning node".into()))?;
	if knowledge::digest(&documents) != expected {
		return Err(Error::Conflict("private document digest mismatch".into()));
	}
	Ok(documents)
}
