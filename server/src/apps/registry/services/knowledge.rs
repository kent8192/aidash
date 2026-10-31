//! HTTP input adaptation for private agent definitions.
pub use crate::apps::registry::serializers::knowledge::{PersonalAgent, ReferenceDocument};
use crate::{Error, Result, federation::Federation, registry::Entry};
use http::HeaderMap;
use reinhardt::injectable;
use serde_json::Value;
use uuid::Uuid;
pub async fn load(
	registry: &crate::registry::Registry,
	tenant: &aidash_domain::tenancy::TenantName,
	entry: &Entry,
) -> Result<Value> {
	registry.private_documents(tenant, entry).await
}

/// Complete provider calls without retaining the Registry write transaction.
pub(crate) async fn embed_before_registration(
	store: &crate::store::Store,
	identity: &crate::authorization::identity::SubjectIdentity,
	reference: &aidash_domain::registry::EntityRef,
	images: &[aidash_domain::registry::knowledge::ValidatedImage],
) -> Result<(Entry, Vec<Vec<f32>>)> {
	use aidash_application::ports::{
		EmbeddingProvider, generation::embedding::GenerationEmbeddingAuthority,
	};
	use aidash_application::provider_access::MaintenancePurpose;
	use aidash_domain::semantic::{EmbeddingConfig, EmbeddingInput};
	let mut pinned: Option<Entry> = None;
	let mut vectors = Vec::with_capacity(images.len());
	for image in images {
		let mut access = crate::authorization::access::Access::begin(store, identity).await?;
		let prepared: Result<_> = async {
			let entry =
				crate::authorization::catalog::entry(&mut access, reference, "registry.read")
					.await?;
			// Sending images to the provider is an invocation, independent of reading the entry.
			crate::authorization::catalog::entry(&mut access, reference, "embedding.invoke")
				.await?;
			aidash_application::generation::embedding::require_reference_policy(
				&mut crate::bootstrap::generation_embedding_authority_scope(&mut access),
				store.deployment.as_str(),
				reference,
			)
			.await?;
			if entry.kind != "embedding" {
				return Err(Error::Invalid(
					"reference images require an embedding definition".into(),
				));
			}
			let config: EmbeddingConfig = serde_json::from_value(entry.config.clone())?;
			let input = EmbeddingInput::Image {
				media_type: &image.media_type,
				bytes: &image.bytes,
			};
			input.validate(&config)?;
			let jobs = crate::bootstrap::generation_embedding_authority_scope(&mut access)
				.requests(store.deployment.as_str())
				.await?;
			let workspace = jobs
				.first()
				.map(|job| job.workspace_id)
				.unwrap_or(Uuid::nil());
			if jobs.iter().any(|job| job.workspace_id != workspace) {
				return Err(Error::Forbidden);
			}
			let reservation = crate::generation::embedding::reserve_input(
				&mut access,
				store,
				workspace,
				&config,
				input,
				crate::generation::embedding::Origin::Reference,
			)
			.await?;
			Ok((entry, config, reservation))
		}
		.await;
		let (entry, config, reservation) = access.finish(prepared).await?;
		if pinned.as_ref().is_some_and(|previous| previous != &entry) {
			return Err(Error::Conflict(
				"reference embedding definition changed".into(),
			));
		}
		let transport = if config.provider_credential.is_some() {
			crate::bootstrap::admitted_semantic_transport(
				store,
				None,
				identity.tenant.clone(),
				Some(MaintenancePurpose::ReferenceIndexing),
			)
		} else {
			crate::bootstrap::semantic_transport(store)
		};
		let output = transport
			.embed(
				&config,
				EmbeddingInput::Image {
					media_type: &image.media_type,
					bytes: &image.bytes,
				},
			)
			.await?;
		if let Some(reservation) = reservation {
			reservation.settle(output.tokens).await?;
		}
		pinned = Some(entry);
		vectors.push(output.vector);
	}
	Ok((
		pinned.ok_or_else(|| Error::Invalid("no reference images".into()))?,
		vectors,
	))
}

/// Distinguish a missing or withdrawn embedding from an independent policy denial.
pub(crate) async fn embedding_available(
	access: &mut crate::authorization::access::Access,
	reference: &aidash_domain::registry::EntityRef,
) -> Result<Option<Entry>> {
	let current = match crate::apps::registry::models::Definition::read_in(
		&mut **access.tx,
		&reference.id,
		&reference.version,
	)
	.await
	{
		Ok(entry) => entry,
		Err(Error::NotFound(_)) => return Ok(None),
		Err(error) => return Err(error),
	};
	if !aidash_application::registry::system::builtin_reference(reference) {
		let tenant = access.identity.tenant.clone();
		let node = access.catalog_node().await?;
		if crate::apps::identity::models::AuthorizationCatalog::enabled_in(
			&mut **access.tx,
			&tenant,
			node.as_str(),
			reference,
			true,
		)
		.await? != Some(true)
		{
			return Ok(None);
		}
	}
	Ok(Some(current))
}

/// Search only the executing Agent version's private collection using its pinned model.
pub(crate) async fn reference_image_context(
	access: &mut crate::authorization::access::Access,
	store: &crate::store::Store,
	run: &crate::domain::Run,
	query: aidash_domain::semantic::EmbeddingInput<'_>,
	limit: usize,
) -> Result<Option<Value>> {
	use aidash_domain::registry::{
		bindings::{
			AgentBindings,
			sources::{NativeSource, validate_definition},
		},
		knowledge,
	};
	let snapshot = run
		.context
		.binding_snapshot
		.as_deref()
		.ok_or_else(|| Error::Invalid("Run has no admitted Binding snapshot".into()))?;
	let agent = snapshot
		.definitions
		.iter()
		.find(|definition| definition.identity == snapshot.agent)
		.ok_or_else(|| Error::Invalid("Run has no pinned Agent definition".into()))?;
	let bindings: AgentBindings = serde_json::from_value(agent.definition.config.clone())?;
	let Some(reference) = bindings.reference_embedding else {
		return Ok(None);
	};
	let unavailable = || {
		Error::Invalid("reference embedding is unavailable; register a new agent version".into())
	};
	let current = embedding_available(access, &reference)
		.await?
		.ok_or_else(unavailable)?;
	let approved =
		crate::authorization::catalog::entry(access, &reference, "registry.read").await?;
	crate::authorization::catalog::entry(access, &reference, "embedding.invoke").await?;
	aidash_application::generation::embedding::require_reference_policy(
		&mut crate::bootstrap::generation_embedding_authority_scope(access),
		store.deployment.as_str(),
		&reference,
	)
	.await?;
	let pinned = snapshot
		.definitions
		.iter()
		.find(|definition| {
			definition.identity.id == reference.id
				&& definition.identity.version == reference.version
				&& definition.identity.registry_deployment == snapshot.agent.registry_deployment
		})
		.ok_or_else(unavailable)?;
	if current != approved || current != pinned.definition {
		return Err(unavailable());
	}
	let mut collections = Vec::new();
	for binding in snapshot
		.bindings
		.iter()
		.filter(|binding| binding.excluded_reason.is_none() && binding.definition.kind == "source")
	{
		let Some(context) = validate_definition(&binding.definition)? else {
			continue;
		};
		let NativeSource::PrivateReferences { digest } = context.source else {
			continue;
		};
		let documents = crate::apps::registry::models::transaction_records::documents(
			&mut **access.tx,
			&binding.definition,
		)
		.await?
		.ok_or_else(unavailable)?;
		if knowledge::digest(&documents) != digest {
			return Err(unavailable());
		}
		let stored: Vec<aidash_domain::registry::ReferenceDocument> =
			serde_json::from_value(documents)?;
		let index = crate::apps::registry::models::transaction_records::image_index(
			&mut **access.tx,
			&binding.definition,
		)
		.await?;
		let Some(index) = index else {
			if stored
				.iter()
				.any(|document| document.image_digest.is_some())
			{
				return Err(unavailable());
			}
			continue;
		};
		let collection: knowledge::ImageCollection = serde_json::from_value(index)?;
		if collection.agent_id != snapshot.agent.id
			|| collection.agent_version != snapshot.agent.version
		{
			return Err(Error::Forbidden);
		}
		collection
			.verify(
				&snapshot.agent.id,
				&snapshot.agent.version,
				&approved,
				&stored,
			)
			.map_err(|_| unavailable())?;
		collections.push((binding.identity.clone(), collection));
	}
	if collections.is_empty() {
		return Ok(None);
	}
	let config: aidash_domain::semantic::EmbeddingConfig =
		serde_json::from_value(approved.config.clone())?;
	let vector = crate::semantic::service::embed_input(
		store,
		&mut crate::semantic::service::Lease::Inherited(access),
		run.workspace_id,
		&config,
		query,
		crate::generation::embedding::Origin::ReferenceQuery(run.id),
	)
	.await
	.map_err(|error| match error {
		Error::RemoteSemantic(aidash_domain::semantic::Failure::EmbeddingModelNotFound) => {
			unavailable()
		}
		other => other,
	})?;
	let mut matches = Vec::new();
	for (source, collection) in collections {
		for found in collection.search(
			&snapshot.agent.id,
			&snapshot.agent.version,
			&vector,
			limit.min(8),
		)? {
			matches.push((source.clone(), found));
		}
	}
	Ok(Some(Value::Array(
		knowledge::rank_matches(matches, limit.min(8))
			.into_iter()
			.map(|(source, document)| serde_json::json!({"source":source, "document":document}))
			.collect(),
	)))
}
#[derive(Clone)]
pub struct PersonalAgents {
	pub(crate) runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide_personal_agents(#[inject] runtime: Federation) -> PersonalAgents {
	PersonalAgents { runtime }
}
impl PersonalAgents {
	/// An agent version with its Reference Documents, under `registry.create`.
	pub(crate) async fn create(
		&self,
		actor: crate::authorization::identity::Actor,
		headers: HeaderMap,
		input: PersonalAgent,
	) -> Result<Entry> {
		let crate::authorization::identity::Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let draft =
			aidash_application::registry::personal::validate_input(input.entry, input.documents)?;
		let key = headers
			.get("idempotency-key")
			.and_then(|value| value.to_str().ok())
			.and_then(|value| Uuid::parse_str(value).ok())
			.ok_or_else(|| Error::Invalid("Idempotency-Key must be a UUID".into()))?;
		let f = &self.runtime;
		let mut access = crate::authorization::access::Access::begin(&f.store, &identity).await?;
		let preflight: Result<_> = async {
			let mut scope = crate::apps::registry::repositories::requests::TenantRegistration {
				access: &mut access,
				store: &f.store,
			};
			let registration = aidash_application::registry::personal::prepare(
				&mut scope,
				&f.registry.validation(),
				draft,
				f.config.deployment.as_str(),
			)
			.await?;
			let replay = scope.replay(&registration.entry, key).await?;
			Ok((registration, replay))
		}
		.await;
		let (registration, replay) = access.finish(preflight).await?;
		if let Some(entry) = replay {
			return Ok(entry);
		}
		let prepared = if let Some((reference, images)) =
			aidash_application::registry::personal::reference_images(&registration)?
		{
			Some(embed_before_registration(&f.store, &identity, &reference, &images).await?)
		} else {
			None
		};
		let mut access = crate::authorization::access::Access::begin(&f.store, &identity).await?;
		let result = register(f, &mut access, registration, prepared, key).await;
		access.finish(result).await
	}
}

async fn register(
	f: &Federation,
	access: &mut crate::authorization::access::Access,
	registration: aidash_application::registry::personal::PersonalRegistration,
	prepared: Option<(Entry, Vec<Vec<f32>>)>,
	key: Uuid,
) -> Result<Entry> {
	if let Some((embedding, _)) = &prepared {
		let reference = aidash_domain::registry::EntityRef {
			id: embedding.id.clone(),
			version: embedding.version.clone(),
		};
		let current =
			crate::authorization::catalog::entry(access, &reference, "registry.read").await?;
		if current != *embedding {
			return Err(Error::Conflict(
				"reference embedding definition changed".into(),
			));
		}
		aidash_application::generation::embedding::require_reference_policy(
			&mut crate::bootstrap::generation_embedding_authority_scope(access),
			f.store.deployment.as_str(),
			&reference,
		)
		.await?;
	}
	let validation = f.registry.validation();
	let mut scope = crate::apps::registry::repositories::requests::TenantRegistration {
		access,
		store: &f.store,
	};
	Ok(aidash_application::registry::personal::register(
		&mut scope,
		&validation,
		registration,
		prepared,
		key,
		f.config.deployment.as_str(),
	)
	.await?)
}
