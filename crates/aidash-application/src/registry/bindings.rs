//! Resolve a complete capability closure before Run activation, never a partial tool set.
use crate::{
	Error, Result,
	ports::bindings::{BindingCatalog, ProviderCatalog},
};
use aidash_domain::{
	registry::{Entry, bindings::*},
	tool::providers::{ToolDescriptor, remote_exclusion, reserved_aliases, validate_restrictions},
};
use std::collections::{BTreeMap, BTreeSet};
pub mod catalog;
pub mod private;

struct Pending {
	normalized: NormalizedBinding,
	ancestry: BTreeSet<QualifiedRef>,
	lifecycle: Option<(String, String)>,
}
fn contract_digest(contract: &aidash_domain::tool::ToolContract) -> Result<String> {
	Ok(aidash_domain::registry::rules::digest(
		&serde_json::to_value(contract)?,
	))
}
pub async fn resolve(
	catalog: &mut dyn BindingCatalog,
	providers: &dyn ProviderCatalog,
	agent: QualifiedRef,
	agent_definition: &Entry,
	remote: bool,
) -> Result<BindingSnapshot> {
	if agent_definition.id != agent.id
		|| agent_definition.version != agent.version
		|| agent_definition.kind != "agent"
	{
		return Err(Error::Invalid(
			"Agent admission received a different definition".into(),
		));
	}
	let config: AgentBindings = serde_json::from_value(agent_definition.config.clone())?;
	if let Some(installation) = &agent_definition.installation {
		catalog.installation(installation).await?;
	}
	let mut agent_definition = agent_definition.clone();
	if agent_definition.binding_normalization.is_none() {
		agent_definition.normalize_agent(&agent.registry_node)?;
	}
	let normalized = agent_definition.normalized_bindings(&agent.registry_node)?;
	let mut pending: Vec<_> = normalized
		.into_iter()
		.map(|normalized| Pending {
			normalized,
			ancestry: BTreeSet::new(),
			lifecycle: None,
		})
		.collect();
	let mut definitions: BTreeMap<QualifiedRef, Entry> = BTreeMap::new();
	definitions.insert(agent.clone(), agent_definition.clone());
	let mut resolved: BTreeMap<QualifiedRef, ResolvedBinding> = BTreeMap::new();
	let mut aliases = BTreeSet::new();
	let mut operations = BTreeSet::new();
	let mut source_skill_support = false;
	for (reference, kind) in std::iter::once((&config.model, "model"))
		.chain(config.cluster.iter().map(|r| (r, "cluster")))
	{
		let qualified = QualifiedRef {
			registry_node: agent.registry_node.clone(),
			id: reference.id.clone(),
			version: reference.version.clone(),
		};
		let entry = catalog.definition(&qualified).await?;
		if entry.kind != kind {
			return Err(Error::Invalid(format!(
				"{} must reference a {kind}",
				reference.id
			)));
		}
		if let Some(installation) = &entry.installation {
			catalog.installation(installation).await?;
		}
		if definitions.insert(qualified, entry).is_some() {
			return Err(Error::Invalid(
				"Agent model or cluster collides with its own identity".into(),
			));
		}
	}
	while let Some(Pending {
		normalized,
		mut ancestry,
		lifecycle,
	}) = pending.pop()
	{
		let Binding {
			kind,
			target,
			alias,
			narrow,
			members,
		} = normalized.binding;
		let origin = normalized.origin;
		target.validate()?;
		let entry = if let Some(entry) = definitions.get(&target) {
			entry.clone()
		} else {
			catalog.definition(&target).await?
		};
		if entry.id != target.id || entry.version != target.version {
			return Err(Error::Invalid(
				"qualified definition lookup returned a different identity".into(),
			));
		}
		if let Some(projection) = &entry.installation {
			catalog.installation(projection).await?;
		}
		definitions.insert(target.clone(), entry.clone());
		if definitions.len() > MAX_BINDINGS || resolved.len() >= MAX_BINDINGS {
			return Err(Error::Invalid(
				"Binding dependency closure exceeds 128 definitions".into(),
			));
		}
		if kind == BindingKind::Bundle {
			if entry.kind != "bundle" || !ancestry.insert(target.clone()) {
				return Err(Error::Invalid(
					"invalid or recursive bundle expansion".into(),
				));
			}
			let bundle: BundleConfig = serde_json::from_value(entry.config.clone())?;
			bundle.validate()?;
			if members
				.iter()
				.any(|id| !bundle.members.iter().any(|member| &member.id == id))
			{
				return Err(Error::Invalid(
					"bundle member selection names an undeclared operation".into(),
				));
			}
			for member in bundle
				.members
				.into_iter()
				.filter(|r| members.is_empty() || members.contains(&r.id))
			{
				let definition = catalog.definition(&member).await?;
				let member_kind = match definition.kind.as_str() {
					"tool" => BindingKind::Tool,
					"bundle" => BindingKind::Bundle,
					_ => {
						return Err(Error::Invalid(
							"bundle members must be Tools or bundles".into(),
						));
					}
				};
				definitions.insert(member.clone(), definition);
				pending.push(Pending {
					normalized: NormalizedBinding {
						binding: Binding {
							kind: member_kind,
							target: member,
							alias: None,
							narrow: narrow.clone(),
							members: vec![],
						},
						origin,
					},
					ancestry: ancestry.clone(),
					lifecycle: None,
				});
			}
			continue;
		}
		let mut provider_contract_digest = None;
		let mut provider_implementation = None;
		let mut excluded_reason = None;
		let mut effective_alias = alias;
		let mut effective_narrow = narrow;
		if kind == BindingKind::Tool {
			if entry.kind != "tool" {
				return Err(Error::Invalid(
					"Tool Binding requires a Tool descriptor".into(),
				));
			}
			let descriptor: ToolDescriptor = serde_json::from_value(entry.config.clone())?;
			let contract = providers.contract(&descriptor, &target)?;
			validate_restrictions(&descriptor.operation, &effective_narrow)?;
			effective_narrow = descriptor.narrow.intersect(&effective_narrow)?;
			if let Some((provider, operation)) = &lifecycle
				&& (descriptor.provider != *provider
					|| descriptor.operation != *operation
					|| descriptor.lifecycle.is_some())
			{
				return Err(Error::Invalid(
					"lifecycle companion does not match its admitted provider operation".into(),
				));
			}
			let name = effective_alias.get_or_insert_with(|| descriptor.default_alias.clone());
			validate_alias(name)?;
			if reserved_aliases().contains(name.as_str()) && name != &descriptor.operation {
				return Err(Error::Invalid(
					"tool alias impersonates a reserved operation".into(),
				));
			}
			if origin == BindingOrigin::Required && name != &descriptor.operation {
				return Err(Error::Invalid(
					"mandatory tools retain their canonical aliases".into(),
				));
			}
			if remote {
				excluded_reason = remote_exclusion(origin, &contract)?;
			}
			provider_contract_digest = Some(contract_digest(&contract)?);
			if excluded_reason.is_none() {
				provider_implementation = Some(providers.implementation(&descriptor)?);
			}
			if excluded_reason.is_none() {
				operations.insert(descriptor.operation.clone());
			}
			if let Some(companions) = descriptor.lifecycle {
				for (companion, suffix) in
					[(companions.poll, "poll"), (companions.cancel, "cancel")]
				{
					let operation = if descriptor.operation == "shell" {
						format!("shell_{suffix}")
					} else {
						format!("python_{suffix}")
					};
					pending.push(Pending {
						normalized: NormalizedBinding {
							binding: Binding::tool(companion),
							origin: BindingOrigin::Companion,
						},
						ancestry: BTreeSet::new(),
						lifecycle: Some((descriptor.provider.clone(), operation)),
					});
				}
			}
		} else {
			let expected = match kind {
				BindingKind::Skill => "skill",
				BindingKind::Memory => "memory",
				BindingKind::Source => "source",
				_ => unreachable!(),
			};
			if entry.kind != expected {
				return Err(Error::Invalid(format!(
					"Binding requires a {expected} definition"
				)));
			}
			if effective_narrow != Narrowing::default() {
				return Err(Error::Invalid(
					"native context restriction is not supported by this adapter".into(),
				));
			}
			if matches!(kind, BindingKind::Memory | BindingKind::Source)
				&& let Some(descriptor) = sources::validate_definition(&entry)?
			{
				if remote
					&& matches!(
						descriptor.source,
						sources::NativeSource::ReferenceAttachments { .. }
							| sources::NativeSource::SkillAttachments { .. }
							| sources::NativeSource::SkillRoots { .. }
					) {
					return Err(Error::Invalid(
						"native mounted Source is unavailable remotely".into(),
					));
				}
				source_skill_support |= descriptor.requires_skill_support();
			}
			catalog.source(&entry).await?;
		}
		let binding = ResolvedBinding {
			identity: target.clone(),
			digest: aidash_domain::registry::rules::digest(&serde_json::to_value(&entry)?),
			installation: entry.installation.clone(),
			definition: entry,
			origin,
			alias: effective_alias,
			narrow: effective_narrow,
			provider_contract_digest,
			provider_implementation,
			excluded_reason,
		};
		if let Some(existing) = resolved.get(&target) {
			if existing.origin == BindingOrigin::Companion || origin == BindingOrigin::Companion {
				let mut same = binding.clone();
				same.origin = existing.origin;
				if existing == &same {
					// A bundle may enumerate the same poll/cancel declaration that a
					// start operation generates. Preserve an explicit member's origin.
					if existing.origin == BindingOrigin::Companion
						&& origin != BindingOrigin::Companion
					{
						resolved.insert(target, binding);
					}
					continue;
				}
			}
			return Err(Error::Invalid(
				"conflicting expanded Binding for an exact capability".into(),
			));
		}
		if binding.excluded_reason.is_none()
			&& let Some(alias) = &binding.alias
			&& !aliases.insert(alias.clone())
		{
			return Err(Error::Invalid(format!(
				"duplicate resolved tool alias: {alias}"
			)));
		}
		resolved.insert(target, binding);
	}
	if source_skill_support {
		for operation in SKILL_TOOLS {
			let identity = QualifiedRef::builtin(&agent.registry_node, operation);
			let support = resolved.get_mut(&identity).ok_or_else(|| {
				Error::Invalid("native Skills require all Skill support tools".into())
			})?;
			if support.alias.as_deref() != Some(operation) {
				return Err(Error::Invalid(
					"native Skill support tools retain their canonical aliases".into(),
				));
			}
			if support.excluded_reason.is_some() {
				return Err(Error::Invalid(
					"native Skill support is unavailable at this placement".into(),
				));
			}
			if support.origin == BindingOrigin::Default {
				support.origin = BindingOrigin::SkillSupport;
			}
		}
	}
	if config.instructions.trim().is_empty()
		&& !source_skill_support
		&& !config
			.bindings
			.iter()
			.any(|binding| binding.kind == BindingKind::Skill)
	{
		return Err(Error::Invalid(
			"Agent requires instructions or a bound Skill context".into(),
		));
	}
	if config.cluster.is_some()
		&& COORDINATOR_TOOLS
			.iter()
			.any(|name| !operations.contains(*name))
	{
		return Err(Error::Invalid(
			"cluster coordinator requires coordination operations".into(),
		));
	}
	let mut queue = definitions
		.iter()
		.map(|(r, e)| (r.clone(), e.clone()))
		.collect::<Vec<_>>();
	let mut visited = BTreeSet::new();
	let mut foreign_agents = BTreeMap::new();
	while let Some((identity, entry)) = queue.pop() {
		if !visited.insert(identity.clone()) {
			continue;
		}
		for (reference, kind) in definition_references(&identity, &entry)? {
			if reference.registry_node != identity.registry_node
				&& kind == "agent"
				&& !foreign_agents.contains_key(&reference)
			{
				let snapshot = catalog.foreign_agent(&reference).await?;
				snapshot.validate()?;
				if snapshot.agent != reference {
					return Err(Error::Invalid(
						"foreign Agent closure returned a different root".into(),
					));
				}
				for pinned in &snapshot.definitions {
					if let Some(existing) = definitions.get(&pinned.identity) {
						if existing != &pinned.definition {
							return Err(Error::Conflict(
								"foreign Agent closures disagree on an immutable definition".into(),
							));
						}
					} else {
						definitions.insert(pinned.identity.clone(), pinned.definition.clone());
						if definitions.len() > MAX_BINDINGS {
							return Err(Error::Invalid(
								"Binding dependency closure exceeds 128 definitions".into(),
							));
						}
					}
				}
				foreign_agents.insert(reference.clone(), snapshot);
			}
			let dependency = if let Some(entry) = definitions.get(&reference) {
				entry.clone()
			} else {
				let entry = catalog.definition(&reference).await?;
				if let Some(installation) = &entry.installation {
					catalog.installation(installation).await?;
				}
				if reference.id != entry.id || reference.version != entry.version {
					return Err(Error::Invalid(
						"dependency lookup returned a different identity".into(),
					));
				}
				definitions.insert(reference.clone(), entry.clone());
				if definitions.len() > MAX_BINDINGS {
					return Err(Error::Invalid(
						"Binding dependency closure exceeds 128 definitions".into(),
					));
				}
				entry
			};
			if !reference_kind_matches(&kind, &dependency.kind) {
				return Err(Error::Invalid(
					"Binding dependency has a different Registry kind".into(),
				));
			}
			queue.push((reference, dependency));
		}
	}
	validate_bundle_graph(&definitions)?;
	for (identity, definition) in &definitions {
		if definition.kind == "cluster" {
			let cluster: aidash_domain::registry::ClusterConfig =
				serde_json::from_value(definition.config.clone())?;
			let coordinator = QualifiedRef {
				registry_node: identity.registry_node.clone(),
				id: cluster.coordinator.id,
				version: cluster.coordinator.version,
			};
			let entry = &definitions[&coordinator];
			let configured: AgentBindings = serde_json::from_value(entry.config.clone())?;
			let mut pending = configured
				.normalize(&coordinator.registry_node)?
				.into_iter()
				.map(|n| n.binding)
				.collect::<Vec<_>>();
			let mut operations = BTreeSet::new();
			let mut seen = BTreeSet::new();
			while let Some(binding) = pending.pop() {
				if !seen.insert(binding.target.clone()) {
					continue;
				}
				let entry = &definitions[&binding.target];
				match binding.kind {
					BindingKind::Bundle => {
						let bundle: BundleConfig = serde_json::from_value(entry.config.clone())?;
						bundle.validate()?;
						if binding
							.members
							.iter()
							.any(|id| !bundle.members.iter().any(|member| &member.id == id))
						{
							return Err(Error::Invalid(
								"bundle member selection names an undeclared operation".into(),
							));
						}
						for member in bundle.members.into_iter().filter(|r| {
							binding.members.is_empty() || binding.members.contains(&r.id)
						}) {
							let kind = if definitions[&member].kind == "bundle" {
								BindingKind::Bundle
							} else {
								BindingKind::Tool
							};
							pending.push(Binding {
								kind,
								target: member,
								alias: None,
								narrow: Default::default(),
								members: vec![],
							});
						}
					}
					BindingKind::Tool => {
						let descriptor: ToolDescriptor =
							serde_json::from_value(entry.config.clone())?;
						providers.contract(&descriptor, &binding.target)?;
						operations.insert(descriptor.operation);
					}
					_ => {}
				}
			}
			if COORDINATOR_TOOLS
				.iter()
				.any(|operation| !operations.contains(*operation))
			{
				return Err(Error::Invalid(
					"cluster coordinator lacks required coordination operations".into(),
				));
			}
		}
	}
	let definitions = definitions
		.into_iter()
		.map(|(identity, definition)| ResolvedDefinition::new(identity, definition))
		.collect::<std::result::Result<Vec<_>, _>>()?;
	let snapshot = BindingSnapshot {
		schema_version: BINDING_SCHEMA,
		agent,
		remote,
		bindings: resolved.into_values().collect(),
		definitions,
		foreign_agents: foreign_agents.into_values().collect(),
	};
	snapshot.validate()?;
	let settings = aidash_domain::registry::AgentConfig::from_snapshot(&snapshot)?;
	validate_memory_sources(&snapshot, &settings)?;
	Ok(snapshot)
}

#[cfg(test)]
mod tests;

/// Used at execution boundaries; current authority is still required by the native adapter.
pub fn recheck_provider(
	providers: &dyn ProviderCatalog,
	binding: &ResolvedBinding,
) -> Result<aidash_domain::tool::ToolContract> {
	let descriptor: ToolDescriptor = serde_json::from_value(binding.definition.config.clone())?;
	let contract = providers.contract(&descriptor, &binding.identity)?;
	if binding.provider_contract_digest.as_deref() != Some(&contract_digest(&contract)?) {
		return Err(Error::Conflict(
			"pinned provider operation contract changed".into(),
		));
	}
	providers.implementation(&descriptor)?;
	Ok(contract)
}

pub mod execution;

pub(crate) fn validate_bundle_graph(definitions: &BTreeMap<QualifiedRef, Entry>) -> Result<()> {
	fn visit(
		identity: &QualifiedRef,
		definitions: &BTreeMap<QualifiedRef, Entry>,
		path: &mut BTreeSet<QualifiedRef>,
		done: &mut BTreeSet<QualifiedRef>,
	) -> Result<()> {
		if done.contains(identity) || definitions[identity].kind != "bundle" {
			return Ok(());
		}
		if !path.insert(identity.clone()) {
			return Err(Error::Invalid("recursive bundle dependency".into()));
		}
		let bundle: BundleConfig = serde_json::from_value(definitions[identity].config.clone())?;
		for member in bundle.members {
			visit(&member, definitions, path, done)?;
		}
		path.remove(identity);
		done.insert(identity.clone());
		Ok(())
	}
	let mut done = BTreeSet::new();
	for identity in definitions.keys() {
		visit(identity, definitions, &mut BTreeSet::new(), &mut done)?;
	}
	Ok(())
}

/// Component decisions accompany Registry disclosure; they do not imply resource authority.
pub fn component_action(kind: &str) -> &'static str {
	match kind {
		"agent" => "agent.execute",
		"model" => "model.infer",
		"tool" => "tool.invoke",
		"skill" => "skill.use",
		"cluster" => "cluster.execute",
		"memory" | "source" => "registry.read",
		_ => "registry.read",
	}
}

fn validate_memory_sources(
	snapshot: &BindingSnapshot,
	settings: &aidash_domain::registry::AgentConfig,
) -> Result<()> {
	use aidash_domain::memory::{ProviderConfig, SourceConfig, SourceScope};
	if settings.memory.is_none() && !settings.sources.is_empty() {
		return Err(Error::Invalid(
			"Agent Sources require a primary memory provider".into(),
		));
	}
	if settings.sources.len() > 32 {
		return Err(Error::Invalid(
			"Agent admits at most 32 memory Sources".into(),
		));
	}
	let lookup = |reference: &aidash_domain::registry::EntityRef| {
		snapshot
			.definitions
			.iter()
			.find(|d| {
				d.identity.registry_node == snapshot.agent.registry_node
					&& d.identity.local() == *reference
			})
			.map(|d| &d.definition)
			.ok_or_else(|| {
				Error::Invalid("memory dependency is absent from the admitted closure".into())
			})
	};
	let mut workspace_provider = None;
	let mut providers: Vec<_> = settings.memory.iter().cloned().collect();
	for reference in &settings.sources {
		let source: SourceConfig = serde_json::from_value(lookup(reference)?.config.clone())?;
		if source.scope == SourceScope::Participant
			&& settings.memory.as_ref() != Some(&source.memory)
		{
			return Err(Error::Invalid(
				"participant Sources require the Agent's exact primary memory provider".into(),
			));
		}
		if source.scope == SourceScope::Workspace {
			if workspace_provider
				.as_ref()
				.is_some_and(|p| p != &source.memory)
			{
				return Err(Error::Invalid(
					"workspace Sources require one exact memory provider version".into(),
				));
			}
			workspace_provider = Some(source.memory.clone());
		}
		let provider: ProviderConfig =
			serde_json::from_value(lookup(&source.memory)?.config.clone())?;
		if source.max_tokens > provider.policy.bounds.max_context_tokens {
			return Err(Error::Invalid(
				"source context cap exceeds its memory provider cap".into(),
			));
		}
		providers.push(source.memory);
	}
	let mut embedding = None;
	for reference in providers {
		let provider: ProviderConfig = serde_json::from_value(lookup(&reference)?.config.clone())?;
		let configuration: aidash_domain::semantic::EmbeddingConfig =
			serde_json::from_value(lookup(&provider.policy.embedding)?.config.clone())?;
		if embedding
			.as_ref()
			.is_some_and(|previous| previous != &configuration)
		{
			return Err(Error::Invalid(
				"an Agent's memory providers require compatible embedding configurations".into(),
			));
		}
		aidash_domain::memory::graph::validate_capacity(
			&provider.policy.bounds,
			configuration.dimensions,
		)?;
		embedding = Some(configuration);
	}
	Ok(())
}
