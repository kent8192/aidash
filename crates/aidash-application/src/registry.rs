//! Registry discovery, admission, publication and immutable Run binding.
use crate::{
	Error, Result,
	ports::registry::{
		DefinitionLookup, DefinitionWriter, PackageScope, PackageSnapshot, RegistrationScope,
		RegistryRead,
	},
};
use aidash_domain::registry::rules::{digest, overlay_config, validate_override_keys};
use aidash_domain::{
	registry::{AgentPage, ClusterConfig, Entry, Package, PackageRecord, Search},
	tool::ToolConfig,
};
use serde_json::{Value, json};
use uuid::Uuid;
pub mod bindings;
pub mod system;
pub mod validation;
pub use validation::DefinitionValidation;

pub async fn overlay(scope: &mut dyn DefinitionLookup, mut entry: Entry) -> Result<Entry> {
	if let Some(config) = scope.overrides(&entry.id, &entry.version).await? {
		if system::is_builtin(&entry) {
			return Err(Error::Conflict(
				"system builtin has an unauthorized configuration overlay".into(),
			));
		}
		overlay_config(&mut entry.config, &config)?;
	}
	Ok(entry)
}
pub async fn effective(scope: &mut dyn DefinitionLookup, id: &str, version: &str) -> Result<Entry> {
	let entry = scope.definition(id, version).await?;
	overlay(scope, entry).await
}
pub async fn list(scope: &mut dyn RegistryRead, search: &Search) -> Result<Vec<Entry>> {
	let rows = scope.definitions(search.kind.as_deref(), 0, None).await?;
	let mut entries = Vec::new();
	for row in rows {
		let entry = overlay(scope, serde_json::from_value(row.metadata)?).await?;
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

pub async fn legacy_agents(
	scope: &mut dyn RegistryRead,
	search: &Search,
	offset: u64,
) -> Result<AgentPage> {
	let mut cursor = offset;
	let mut entries = vec![];
	let mut bytes = 0;
	loop {
		let rows = scope
			.definitions(
				Some("agent"),
				usize::try_from(cursor)
					.map_err(|_| Error::Invalid("agent offset exceeds platform limits".into()))?,
				Some(64),
			)
			.await?;
		let exhausted = rows.len() < 64;
		for row in rows {
			if row.metadata.get("installation").is_some()
				|| scope.generated(&row.id, &row.version).await?
			{
				cursor += 1;
				continue;
			}
			let entry = overlay(scope, serde_json::from_value(row.metadata)?).await?;
			if !search.matches(&entry) {
				cursor += 1;
				continue;
			}
			let size = serde_json::to_vec(&entry)?.len() + 1;
			if bytes + size > 3_000_000 {
				if entries.is_empty() {
					return Err(Error::Invalid(
						"agent metadata exceeds discovery page limit".into(),
					));
				}
				return Ok(AgentPage {
					entries,
					next_offset: Some(cursor),
				});
			}
			bytes += size;
			cursor += 1;
			entries.push(entry);
			if entries.len() == 64 {
				return Ok(AgentPage {
					entries,
					next_offset: Some(cursor),
				});
			}
		}
		if exhausted {
			return Ok(AgentPage {
				entries,
				next_offset: None,
			});
		}
	}
}

pub async fn validate_references(
	scope: &mut dyn DefinitionLookup,
	validation: &DefinitionValidation,
	entry: &Entry,
	node: &str,
) -> Result<()> {
	validation.validate_in(entry, true)?;
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

pub async fn publish(
	scope: &mut dyn PackageScope,
	validation: &DefinitionValidation,
	mut package: Package,
) -> Result<PackageRecord> {
	system::reject_distribution(&package.entity)?;
	system::reject_owner_definition(&package.entity)?;
	validation.validate_in(&package.entity, true)?;
	if !matches!(
		package.entity.kind.as_str(),
		"agent" | "tool" | "skill" | "bundle" | "memory" | "source"
	) || package.author.trim().is_empty()
	{
		return Err(Error::Invalid(
			"packages require an author and a distributable capability".into(),
		));
	}
	// Publish authored edges only. Registration provenance and implicit defaults
	// belong to the receiving Node and are never portable package content.
	let node = scope.registry_node().to_owned();
	let mut pending = crate::marketplace::definitions::refs(&package.entity, &node)?;
	pending.extend(
		package
			.dependencies
			.iter()
			.cloned()
			.map(|r| (r, String::new())),
	);
	let mut seen = std::collections::BTreeSet::new();
	while let Some((reference, kind)) = pending.pop() {
		if !seen.insert((reference.id.clone(), reference.version.clone())) {
			continue;
		}
		if seen.len() > 128 {
			return Err(Error::Invalid("package closure exceeds 128 entries".into()));
		}
		let entry = scope.definition(&reference.id, &reference.version).await?;
		system::reject_distribution(&entry)?;
		if !kind.is_empty() && entry.kind != kind {
			return Err(Error::Invalid("package dependency kind changed".into()));
		}
		pending.extend(crate::marketplace::definitions::refs(&entry, &node)?);
	}
	package.entity.binding_normalization = None;
	let value = serde_json::to_value(&package)?;
	let source = value.to_string();
	let hash = digest(&value);
	let (stored, inserted) = scope
		.publish(
			&package.entity.id,
			&package.entity.version,
			value,
			&hash,
			&source,
		)
		.await?;
	if inserted {
		scope
			.append_event(
				"package.published",
				json!({"id":stored.id,"version":stored.version}),
			)
			.await?;
	}
	Ok(stored)
}

pub struct InstallPlan {
	pub package: Package,
	pub effective: Entry,
	pub config: Value,
	pub digest: String,
}
pub fn prepare_install(
	snapshot: PackageSnapshot,
	expected: &str,
	config: Value,
) -> Result<InstallPlan> {
	use sha2::{Digest, Sha256};
	let source_digest = format!("sha256:{:x}", Sha256::digest(snapshot.source.as_bytes()));
	let source: Value = serde_json::from_str(&snapshot.source)?;
	if snapshot.digest != expected || source_digest != expected || source != snapshot.manifest {
		return Err(Error::Conflict("package digest changed".into()));
	}
	let package: Package = serde_json::from_value(source)?;
	system::reject_distribution(&package.entity)?;
	validate_override_keys(&package.entity.kind, &config)?;
	let mut effective = package.entity.clone();
	overlay_config(&mut effective.config, &config)?;
	Ok(InstallPlan {
		package,
		effective,
		config,
		digest: expected.into(),
	})
}
pub async fn install(
	scope: &mut dyn PackageScope,
	validation: &DefinitionValidation,
	mut plan: InstallPlan,
	node: &str,
	id: &str,
	version: &str,
) -> Result<Entry> {
	plan.package.entity.normalize_agent(node)?;
	plan.effective.normalize_agent(node)?;
	validate_references(scope, validation, &plan.effective, node).await?;
	for dependency in &plan.package.dependencies {
		scope
			.definition(&dependency.id, &dependency.version)
			.await?;
	}
	if scope
		.install(&plan.package.entity, &plan.digest, plan.config)
		.await?
	{
		scope
			.append_event("package.installed", json!({"id":id,"version":version}))
			.await?;
	}
	Ok(plan.effective)
}

#[cfg(test)]
mod tests;

pub mod personal;

pub mod skill_import;

pub mod workbench;
