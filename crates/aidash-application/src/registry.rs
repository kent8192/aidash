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
	// A Tenant Secret goes only to the hosts it records (ADR 0051).
	if let Some((name, endpoint)) = aidash_domain::tenant_secrets::definition_target(entry) {
		let hosts = scope.tenant_secret_hosts(&name).await?;
		crate::tenant_secrets::admit(&name, &endpoint, hosts.as_deref())?;
	}
	if entry.kind == "bundle" {
		let bundle: aidash_domain::registry::bindings::BundleConfig =
			serde_json::from_value(entry.config.clone())?;
		for member in bundle.members {
			if member.registry_node != node {
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
		if descriptor.registry_node != node {
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
				registry_node: node.into(),
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
			vec![
				(config.policy.extraction, "model"),
				(config.policy.derivation, "model"),
				(config.policy.reflection, "model"),
				(config.policy.embedding, "embedding"),
				(config.policy.reranker, "reranker"),
				(config.policy.tokenizer, "tokenizer"),
			]
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
				registry_node: node.into(),
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
	entry.normalize_agent(node)?;
	if tracked {
		scope.assign_id(&mut entry, key).await?;
		scope.authorize(&entry).await?;
	}
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
	entry.normalize_agent(node)?;
	validate_references(scope, validation, &entry, node).await?;
	scope.insert_definition(&entry).await
}

#[cfg(test)]
mod tests;

pub mod personal;

pub mod agent_plugin_import;

pub mod skill_import;

pub mod workbench;
