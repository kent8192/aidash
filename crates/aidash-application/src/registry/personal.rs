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

/// Validate private image inputs before provider work or Registry writes.
pub fn reference_images(
	registration: &PersonalRegistration,
) -> Result<
	Option<(
		aidash_domain::registry::EntityRef,
		Vec<knowledge::ValidatedImage>,
	)>,
> {
	let documents: Vec<ReferenceDocument> = serde_json::from_value(registration.documents.clone())?;
	let images = knowledge::validated_images(&documents)?;
	if images.is_empty() {
		return Ok(None);
	}
	let config: aidash_domain::registry::bindings::AgentBindings =
		serde_json::from_value(registration.entry.config.clone())?;
	let reference = config.reference_embedding.ok_or_else(|| {
		Error::Invalid("reference images require a pinned embedding definition".into())
	})?;
	Ok(Some((reference, images)))
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
	let visible = knowledge::visible_documents(&draft.documents)?;
	let (canonical, _) = knowledge::canonical_documents(&draft.documents)?;
	let documents = serde_json::to_value(&canonical)?;
	let raw_documents = serde_json::to_value(draft.documents)?;
	let mut entry = draft.entry;
	let source = super::bindings::private::attach(&mut entry, node, &documents)?;
	let context = json!({"reference_documents":visible});

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
			registry_deployment: node.into(),
			id: preview_entry.id.clone(),
			version: entry.version.clone(),
		},
		&preview_entry,
		false,
	)
	.await?;
	validation.bound_prompt_headroom(&snapshot, &context)?;
	entry.binding_normalization = snapshot
		.definitions
		.iter()
		.find(|definition| definition.identity == snapshot.agent)
		.and_then(|definition| definition.definition.binding_normalization.clone());
	Ok(PersonalRegistration {
		entry,
		documents: raw_documents,
		source,
	})
}

/// Reservation, protected references, definition, private text and outbox share one scope.
pub async fn register(
	scope: &mut dyn PrivateKnowledgeScope,
	validation: &DefinitionValidation,
	mut registration: PersonalRegistration,
	prepared: Option<(Entry, Vec<Vec<f32>>)>,
	key: Uuid,
	node: &str,
) -> Result<Entry> {
	let documents: Vec<ReferenceDocument> = serde_json::from_value(registration.documents.clone())?;
	let images = knowledge::validated_images(&documents)?;
	let (canonical, image_data) = knowledge::canonical_documents(&documents)?;
	let mut image_collection = if images.is_empty() {
		None
	} else {
		let config: aidash_domain::registry::bindings::AgentBindings =
			serde_json::from_value(registration.entry.config.clone())?;
		let reference = config.reference_embedding.ok_or_else(|| {
			Error::Invalid("reference images require a pinned embedding definition".into())
		})?;
		let (embedding, vectors) = prepared.ok_or_else(|| {
			Error::Invalid("reference images were not embedded before registration".into())
		})?;
		if embedding.id != reference.id || embedding.version != reference.version {
			return Err(Error::Conflict(
				"reference embedding definition changed".into(),
			));
		}
		// Build and validate the complete collection before the first Registry write.
		Some(knowledge::ImageCollection::new(
			"pending-agent",
			&registration.entry.version,
			embedding,
			&documents,
			vectors,
		)?)
	};
	scope.assign_id(&mut registration.entry, Some(key)).await?;
	if let Some(collection) = &mut image_collection {
		collection.agent_id = registration.entry.id.clone();
		collection.verify(
			&registration.entry.id,
			&registration.entry.version,
			&collection.embedding,
			&canonical,
		)?;
	}
	scope.authorize(&registration.entry).await?;
	let preview_source = registration.source.id.clone();
	let mut input: aidash_domain::registry::bindings::AgentBindings =
		serde_json::from_value(registration.entry.config.clone())?;
	input.bindings.retain(|b| b.target.id != preview_source);
	registration.entry.config = serde_json::to_value(input)?;
	registration.documents = serde_json::to_value(&canonical)?;
	registration.source =
		super::bindings::private::attach(&mut registration.entry, node, &registration.documents)?;
	registration.entry.normalize_agent(node)?;
	// The caller receives the definition as stored, defaults included.
	super::apply_agent_defaults(scope, &mut registration.entry).await;
	scope.authorize(&registration.source).await?;
	register_definition(scope, validation, &registration.source, node).await?;
	let inserted = register_definition(scope, validation, &registration.entry, node).await?;
	scope
		.insert_documents(&registration.source, registration.documents)
		.await?;
	if let Some(collection) = image_collection {
		scope
			.insert_image_data(&registration.source, image_data)
			.await?;
		if let Some(existing) = scope.image_index(&registration.source).await? {
			let existing: knowledge::ImageCollection = serde_json::from_value(existing)?;
			existing.verify(
				&registration.entry.id,
				&registration.entry.version,
				&collection.embedding,
				&canonical,
			)?;
		} else {
			scope
				.insert_image_index(&registration.source, serde_json::to_value(collection)?)
				.await?;
		}
	}
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

/// Automatic references follow each admitted Source before touching its private store.
/// Manual document reads continue through `load`, including OnDemand Sources.
pub async fn automatic_documents(
	scope: &mut dyn PrivateKnowledgeRead,
	bindings: &[aidash_domain::registry::bindings::ResolvedBinding],
) -> Result<Value> {
	use aidash_domain::registry::bindings::sources::{NativeSource, validate_definition};
	let mut documents = Vec::new();
	for binding in bindings
		.iter()
		.filter(|binding| binding.excluded_reason.is_none() && binding.definition.kind == "source")
	{
		let Some(descriptor) = validate_definition(&binding.definition)? else {
			continue;
		};
		if !matches!(descriptor.source, NativeSource::PrivateReferences { .. })
			|| descriptor
				.context_source
				.as_ref()
				.is_some_and(|declaration| {
					matches!(
						declaration.pin,
						aidash_domain::context::source::PinPolicy::OnDemand
					)
				}) {
			continue;
		}
		let value = load(scope, &binding.definition).await?;
		let stored: Vec<ReferenceDocument> = serde_json::from_value(value)?;
		let visible = knowledge::visible_documents(&stored)?;
		documents.extend(
			visible
				.as_array()
				.ok_or_else(|| Error::Invalid("private Source requires a document list".into()))?
				.iter()
				.cloned(),
		);
	}
	Ok(json!(documents))
}
