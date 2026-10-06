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
	registry::{AgentConfig, AgentPage, ClusterConfig, Entry, Package, PackageRecord, Search},
	tool::ToolConfig,
};
use serde_json::{Value, json};
use uuid::Uuid;
pub mod validation;
pub use validation::DefinitionValidation;

pub async fn overlay(scope: &mut dyn DefinitionLookup, mut entry: Entry) -> Result<Entry> {
	if let Some(config) = scope.overrides(&entry.id, &entry.version).await? {
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
/// Installed Runs bind immutable Registry documents. Legacy overlays remain
/// available only to existing native/legacy execution paths.
pub async fn get_for_run(
	scope: &mut dyn DefinitionLookup,
	run: impl Into<aidash_domain::RunMetadata>,
	id: &str,
	version: &str,
) -> Result<Entry> {
	let run = run.into();
	let root = scope.definition(&run.agent_id, &run.agent_version).await?;
	if root.installation.is_none() {
		return effective(scope, id, version).await;
	}
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
	if entry.kind == "agent" {
		let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
		let mut references = Vec::new();
		for (reference, kind) in std::iter::once((&config.model, "model"))
			.chain(config.tools.iter().map(|r| (r, "tool")))
			.chain(config.skills.iter().map(|r| (r, "skill")))
			.chain(config.cluster.iter().map(|r| (r, "cluster")))
			.chain(config.memory.iter().map(|r| (r, "memory")))
			.chain(config.sources.iter().map(|r| (r, "source")))
		{
			let referenced = effective(scope, &reference.id, &reference.version).await?;
			if referenced.kind != kind {
				return Err(Error::Invalid(format!(
					"{} must reference a {kind}",
					reference.id
				)));
			}
			references.push(referenced);
		}
		validation.agent_prompt_headroom(&config, &references, &Value::Null)?;
	}
	let memory_references = match entry.kind.as_str() {
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
	if entry.kind == "tool"
		&& let ToolConfig::Agent { node_id, agent } = serde_json::from_value(entry.config.clone())?
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
	validate_references(scope, validation, entry, node).await?;
	scope.insert_definition(entry).await
}

pub async fn publish(
	scope: &mut dyn PackageScope,
	validation: &DefinitionValidation,
	package: Package,
) -> Result<PackageRecord> {
	validation.validate_in(&package.entity, true)?;
	if !matches!(package.entity.kind.as_str(), "agent" | "tool" | "skill")
		|| package.author.trim().is_empty()
	{
		return Err(Error::Invalid(
			"packages require an author and an agent, tool or skill".into(),
		));
	}
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
	plan: InstallPlan,
	node: &str,
	id: &str,
	version: &str,
) -> Result<Entry> {
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
