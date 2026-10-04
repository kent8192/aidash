//! Private registration keeps document admission and publication atomic.
use super::{DefinitionValidation, effective, register_definition};
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
}

/// Shape validation precedes transport-level idempotency-key parsing.
pub fn validate_input(entry: Entry, documents: Vec<ReferenceDocument>) -> Result<PersonalDraft> {
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
) -> Result<PersonalRegistration> {
	let documents = serde_json::to_value(draft.documents)?;
	let mut entry = draft.entry;
	entry.config["knowledge_digest"] = json!(knowledge::digest(&documents));
	let context = json!({"reference_documents":documents.clone()});
	let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
	let mut references = Vec::new();
	for reference in std::iter::once(&config.model)
		.chain(config.skills.iter())
		.chain(config.tools.iter())
	{
		references.push(effective(scope, &reference.id, &reference.version).await?);
	}
	validation.agent_prompt_headroom(&config, &references, &context)?;
	Ok(PersonalRegistration { entry, documents })
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
	let inserted = register_definition(scope, validation, &registration.entry, node).await?;
	scope
		.insert_documents(&registration.entry, registration.documents)
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
	let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
	let Some(expected) = config.knowledge_digest else {
		return Ok(json!([]));
	};
	let documents = scope.documents(entry).await?.ok_or_else(|| Error::Invalid("private documents are unavailable on this node; run the original agent on its owning node".into()))?;
	if knowledge::digest(&documents) != expected {
		return Err(Error::Conflict("private document digest mismatch".into()));
	}
	Ok(documents)
}
