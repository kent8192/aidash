//! Registry discovery, admission, publication and immutable Run binding.
use crate::{
	Error, Result,
	ports::registry::{DefinitionLookup, DefinitionWriter, RegistrationScope, RegistryRead},
};
use aidash_domain::{
	registry::{ClusterConfig, Entry, Search},
	tool::ToolConfig,
};
use serde_json::{Value, json};
use uuid::Uuid;
pub mod bindings;
pub mod system;
pub mod validation;
pub use validation::DefinitionValidation;

pub async fn list(scope: &mut dyn RegistryRead, search: &Search) -> Result<Vec<Entry>> {
	let rows = scope.definitions(search.kind.as_deref(), 0, None).await?;
	let mut entries = Vec::new();
	for row in rows {
		let entry: Entry = serde_json::from_value(row.metadata)?;
		if search.matches(&entry) {
			entries.push(entry);
		}
	}
	Ok(entries)
}
/// Runtime resource adapters read immutable documents. Executable graph
/// selection is exclusively the persisted Run Binding snapshot.
pub async fn get_for_run(
	scope: &mut dyn DefinitionLookup,
	run: impl Into<aidash_domain::RunMetadata>,
	id: &str,
	version: &str,
) -> Result<Entry> {
	let run = run.into();
	let root = scope.definition(&run.agent_id, &run.agent_version).await?;
	let _ = root;
	scope.definition(id, version).await
}

pub async fn validate_references(
	scope: &mut dyn DefinitionLookup,
	validation: &DefinitionValidation,
	entry: &Entry,
	node: &str,
) -> Result<()> {
	validation.validate_in(entry, true)?;
	if entry.kind == "source"
		&& let Some(aidash_domain::registry::bindings::sources::NativeContext {
			source:
				aidash_domain::registry::bindings::sources::NativeSource::ReferenceAttachments {
					references,
				},
			..
		}) = aidash_domain::registry::bindings::sources::validate_definition(entry)?
	{
		for reference in references {
			scope.require_reference_read(reference.reference_id).await?;
		}
	}
	// A Tenant Secret goes only to the hosts it records (ADR 0051). A personal-only
	// definition names the person's own secret: only the name is validated here and
	// the hosts are enforced against that row at dispatch (ADR 0100).
	if aidash_domain::tenant_secrets::definition_personal_only(entry)
		&& let Some(name) = aidash_domain::tenant_secrets::definition_secret(entry)
	{
		aidash_domain::tenant_secrets::validate_name(&name)?;
	}
	if let Some((name, endpoint)) = aidash_domain::tenant_secrets::definition_shared_target(entry) {
		let hosts = scope.tenant_secret_hosts(&name).await?;
		crate::tenant_secrets::admit(&name, &endpoint, hosts.as_deref())?;
	}
	if entry.kind == "bundle" {
		let bundle: aidash_domain::registry::bindings::BundleConfig =
			serde_json::from_value(entry.config.clone())?;
		for member in bundle.members {
			if member.registry_deployment != node {
				return Err(Error::Forbidden);
			}
			let definition = scope.definition(&member.id, &member.version).await?;
			if !matches!(definition.kind.as_str(), "tool" | "bundle") {
				return Err(Error::Invalid(
					"bundle member must be a Tool or bundle".into(),
				));
			}
		}
	}
	if entry.kind == "tool" && aidash_domain::tool::legacy_config(&entry.config)?.is_none() {
		let descriptor: aidash_domain::tool::providers::ToolDescriptor =
			serde_json::from_value(entry.config.clone())?;
		if descriptor.registry_deployment != node {
			return Err(Error::Forbidden);
		}
		if let Some(lifecycle) = descriptor.lifecycle {
			for (reference, suffix) in [(lifecycle.poll, "poll"), (lifecycle.cancel, "cancel")] {
				let companion = scope.definition(&reference.id, &reference.version).await?;
				let companion: aidash_domain::tool::providers::ToolDescriptor =
					serde_json::from_value(companion.config)?;
				let operation = if descriptor.operation == "shell" {
					format!("shell_{suffix}")
				} else {
					format!("python_{suffix}")
				};
				if companion.provider != descriptor.provider
					|| companion.operation != operation
					|| companion.lifecycle.is_some()
				{
					return Err(Error::Invalid(
						"lifecycle companion differs from the provider operation".into(),
					));
				}
			}
		}
	}
	if entry.kind == "agent" {
		let snapshot = bindings::resolve(
			&mut bindings::catalog::LookupCatalog {
				definitions: scope,
				node,
			},
			validation,
			aidash_domain::registry::bindings::QualifiedRef {
				registry_deployment: node.into(),
				id: entry.id.clone(),
				version: entry.version.clone(),
			},
			entry,
			false,
		)
		.await?;
		validation.bound_prompt_headroom(&snapshot, &Value::Null)?;
	}
	let memory_references = match entry.kind.as_str() {
		"memory" | "source" if entry.config.get("schema_version").is_some() => vec![],
		"memory" => {
			let config: aidash_domain::memory::ProviderConfig =
				serde_json::from_value(entry.config.clone())?;
			let mut references = Vec::new();
			for role in [
				config.policy.extraction,
				config.policy.derivation,
				config.policy.reflection,
			] {
				if let aidash_domain::memory::RoleModel::Entry(reference) = role {
					references.push((reference, "model"));
				}
			}
			if let Some(reference) = config.policy.embedding {
				references.push((reference, "embedding"));
			}
			references.push((config.policy.reranker, "reranker"));
			references.push((config.policy.tokenizer, "tokenizer"));
			references
		}
		"source" => {
			let config: aidash_domain::memory::SourceConfig =
				serde_json::from_value(entry.config.clone())?;
			vec![(config.memory, "memory")]
		}
		"reranker" => match serde_json::from_value::<aidash_domain::memory::RerankerConfig>(
			entry.config.clone(),
		)? {
			aidash_domain::memory::RerankerConfig::Model { model } => vec![(model, "model")],
			aidash_domain::memory::RerankerConfig::Rrf => vec![],
		},
		_ => vec![],
	};
	for (reference, kind) in memory_references {
		let referenced = scope.definition(&reference.id, &reference.version).await?;
		if referenced.kind != kind || referenced.installation.is_some() {
			return Err(Error::Invalid(format!(
				"memory role {} requires an immutable local {kind} definition",
				reference.id
			)));
		}
		validation.validate_in(&referenced, true)?;
		if entry.kind == "memory" && kind == "embedding" {
			let provider: aidash_domain::memory::ProviderConfig =
				serde_json::from_value(entry.config.clone())?;
			let embedding: aidash_domain::semantic::EmbeddingConfig =
				serde_json::from_value(referenced.config.clone())?;
			aidash_domain::memory::graph::validate_capacity(
				&provider.policy.bounds,
				embedding.dimensions,
			)?;
		}
		if entry.kind == "source" {
			let source: aidash_domain::memory::SourceConfig =
				serde_json::from_value(entry.config.clone())?;
			let provider: aidash_domain::memory::ProviderConfig =
				serde_json::from_value(referenced.config)?;
			if source.max_tokens > provider.policy.bounds.max_context_tokens {
				return Err(Error::Invalid(
					"source context cap exceeds its memory provider cap".into(),
				));
			}
		}
	}
	let transport = if entry.kind == "tool" {
		match aidash_domain::tool::legacy_config(&entry.config)? {
			Some(transport) => Some(transport),
			None => {
				serde_json::from_value::<aidash_domain::tool::providers::ToolDescriptor>(
					entry.config.clone(),
				)
				.map_err(|error| Error::Invalid(error.to_string()))?
				.transport
			}
		}
	} else {
		None
	};
	if let Some(ToolConfig::Agent { node_id, agent }) = transport
		&& node_id == node
		&& scope
			.executor_kind(&agent.id, &agent.version)
			.await?
			.as_deref()
			!= Some("agent")
	{
		return Err(Error::Invalid(
			"agent tool executor must reference a local agent".into(),
		));
	}
	if entry.kind == "cluster" {
		let config: ClusterConfig = serde_json::from_value(entry.config.clone())?;
		if scope
			.executor_kind(&config.coordinator.id, &config.coordinator.version)
			.await?
			.as_deref()
			!= Some("agent")
		{
			return Err(Error::Invalid(
				"cluster coordinator must reference an agent".into(),
			));
		}
		let coordinator = scope
			.definition(&config.coordinator.id, &config.coordinator.version)
			.await?;
		let snapshot = bindings::resolve(
			&mut bindings::catalog::LookupCatalog {
				definitions: scope,
				node,
			},
			validation,
			aidash_domain::registry::bindings::QualifiedRef {
				registry_deployment: node.into(),
				id: coordinator.id.clone(),
				version: coordinator.version.clone(),
			},
			&coordinator,
			false,
		)
		.await?;
		for name in ["task_create", "task_delegate", "agent_discover"] {
			snapshot.operation(name)?;
		}
	}
	Ok(())
}

pub async fn register(
	scope: &mut dyn RegistrationScope,
	validation: &DefinitionValidation,
	mut entry: Entry,
	key: Option<Uuid>,
	tracked: bool,
	node: &str,
) -> Result<Entry> {
	system::reject_owner_definition(&entry)?;
	if entry.binding_normalization.is_some() {
		return Err(Error::Invalid("Binding normalization is read-only".into()));
	}
	bindings::normalize_agent(
		&mut bindings::catalog::LookupCatalog {
			definitions: scope,
			node,
		},
		&mut entry,
		node,
	)
	.await?;
	if tracked {
		scope.assign_id(&mut entry, key).await?;
		scope.authorize(&entry).await?;
	}
	// The caller receives the definition as stored, defaults included.
	apply_definition_defaults(scope, &mut entry).await;
	let inserted = register_definition(scope, validation, &entry, node).await?;
	if tracked && inserted {
		scope
			.append_event(
				"registry.registered",
				json!({"id":entry.id,"version":entry.version,"kind":entry.kind}),
			)
			.await?;
	}
	Ok(entry)
}

pub async fn register_definition(
	scope: &mut dyn DefinitionWriter,
	validation: &DefinitionValidation,
	entry: &Entry,
	node: &str,
) -> Result<bool> {
	system::reject_owner_definition(entry)?;
	let mut entry = entry.clone();
	apply_definition_defaults(scope, &mut entry).await;
	bindings::normalize_agent(
		&mut bindings::catalog::LookupCatalog {
			definitions: scope,
			node,
		},
		&mut entry,
		node,
	)
	.await?;
	validate_references(scope, validation, &entry, node).await?;
	scope.insert_definition(&entry).await
}

/// The cache and projection settings a new definition gets when it names none.
/// The values are written into the definition, so its bytes, digest and every
/// Binding snapshot pin them. Stored definitions are never rewritten (ADR 0093):
/// when this id and version are already registered, a field the stored
/// definition omits stays omitted, so replaying a registration made before these
/// defaults remains idempotent. Applying the defaults twice changes nothing, so
/// callers apply them to the entry they return or test, and
/// [`register_definition`] applies them again before the insert.
///
/// - A model on an allowlisted OpenRouter route declares `explicit` caching
///   (ADR 0115).
/// - An Agent on a model that declares Native gets Native with `structured@3`
///   (#532). One that names a Projection Version keeps it, a Native one without
///   a contract gets `structured@3`, and one whose model lacks Native keeps the
///   omitted defaults (`legacy`, `legacy@1`).
/// - A Native Agent on a structured contract gets `prompt_cache: explicit` when
///   its model accepts breakpoints (ADR 0115). `legacy@1` appends per-turn text
///   to `system`, which would end the cached prefix on those turns.
pub async fn apply_definition_defaults(scope: &mut dyn DefinitionLookup, entry: &mut Entry) {
	if !matches!(entry.kind.as_str(), "agent" | "model") || !entry.config.is_object() {
		return;
	}
	// Reads nothing for a model this does not apply to; a config this cannot
	// read is reported by validation.
	if entry.kind == "model"
		&& (entry.config.get("cache_mode").is_some()
			|| !serde_json::from_value::<aidash_domain::model::ModelConfig>(entry.config.clone())
				.is_ok_and(|config| config.explicit_cache_by_default()))
	{
		return;
	}
	let stored = if entry.id.is_empty() {
		None
	} else {
		// Absent or unreadable: registration reports the real error later.
		scope
			.definition(&entry.id, &entry.version)
			.await
			.ok()
			.filter(|stored| stored.kind == entry.kind)
	};
	let stored_omits = |field: &str| {
		stored
			.as_ref()
			.is_some_and(|stored| stored.config.get(field).is_none())
	};
	if entry.kind == "model" {
		if !stored_omits("cache_mode") {
			entry.config["cache_mode"] = json!("explicit");
		}
		return;
	}
	let model = entry.config.get("model").and_then(|model| {
		Some((
			model.get("id")?.as_str()?.to_owned(),
			model.get("version")?.as_str()?.to_owned(),
		))
	});
	// An unknown or invalid model is reported by reference validation.
	let model = match model {
		Some((id, version)) => match scope.definition(&id, &version).await {
			Ok(model) if model.kind == "model" => {
				serde_json::from_value::<aidash_domain::model::ModelConfig>(model.config).ok()
			}
			_ => None,
		},
		None => None,
	};
	if entry.config.get("projection_version").is_none()
		&& !stored_omits("projection_version")
		&& model.as_ref().is_some_and(|config| {
			config.supports_projection(aidash_domain::projection::ProjectionVersion::Native)
		}) {
		entry.config["projection_version"] = json!("native");
	}
	let native = entry.config.get("projection_version") == Some(&json!("native"));
	if native && entry.config.get("harness_contract").is_none() && !stored_omits("harness_contract")
	{
		entry.config["harness_contract"] = json!("structured@3");
	}
	if native
		&& entry.config.get("harness_contract") != Some(&json!("legacy@1"))
		&& entry.config.get("harness_contract").is_some()
		&& entry.config.get("prompt_cache").is_none()
		&& !stored_omits("prompt_cache")
		&& model
			.as_ref()
			.is_some_and(aidash_domain::model::ModelConfig::accepts_cache_breakpoints)
	{
		entry.config["prompt_cache"] = json!("explicit");
	}
}

#[cfg(test)]
mod tests;

pub mod personal;

pub mod agent_plugin_import;

pub mod skill_import;

pub mod workbench;
