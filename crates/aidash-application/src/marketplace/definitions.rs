//! Resolve all dependencies under one current scope; never return a partial graph.
use crate::{Error, Result, ports::marketplace::DefinitionScope, registry::DefinitionValidation};
use aidash_domain::{
	marketplace::definitions::{content, installation_id, manifest, reference, rewrite},
	marketplace::{Dependency, DependencyBinding, Version},
	registry::rules::overlay_config,
	registry::{AgentConfig, ClusterConfig, EntityRef, Entry},
};
use serde_json::Value;
use std::collections::BTreeSet;
pub fn refs(entry: &Entry, node: &str) -> Result<Vec<(EntityRef, String)>> {
	let mut refs = vec![];
	match entry.kind.as_str() {
		"agent" => {
			let config: aidash_domain::registry::bindings::AgentBindings =
				serde_json::from_value(entry.config.clone())?;
			config.validate()?;
			refs.push((config.model, "model".into()));
			refs.extend(
				config
					.cluster
					.into_iter()
					.map(|reference| (reference, "cluster".into())),
			);
			// Only authored edges travel. The receiving Node materializes its
			// own required/default declarations during registration.
			for binding in config.bindings {
				if binding.target.registry_node != node {
					return Err(Error::Forbidden);
				}
				let kind = match binding.kind {
					aidash_domain::registry::bindings::BindingKind::Tool => "tool",
					aidash_domain::registry::bindings::BindingKind::Bundle => "bundle",
					aidash_domain::registry::bindings::BindingKind::Skill => "skill",
					aidash_domain::registry::bindings::BindingKind::Memory => "memory",
					aidash_domain::registry::bindings::BindingKind::Source => "source",
				};
				refs.push((binding.target.local(), kind.into()));
			}
		}
		"tool" if aidash_domain::tool::legacy_config(&entry.config)?.is_some() => {
			if let Some(aidash_domain::tool::ToolConfig::Agent { node_id, agent }) =
				aidash_domain::tool::legacy_config(&entry.config)?
			{
				if node_id != node {
					return Err(Error::Forbidden);
				}
				refs.push((agent, "agent".into()));
			}
		}
		"tool" => {
			let descriptor: aidash_domain::tool::providers::ToolDescriptor =
				serde_json::from_value(entry.config.clone())?;
			if let Some(lifecycle) = descriptor.lifecycle {
				if lifecycle.poll.registry_node != node || lifecycle.cancel.registry_node != node {
					return Err(Error::Forbidden);
				}
				refs.extend(
					[lifecycle.poll, lifecycle.cancel]
						.into_iter()
						.map(|r| (r.local(), "tool".into())),
				);
			}
			if let Some(aidash_domain::tool::ToolConfig::Agent { node_id, agent }) =
				descriptor.transport
			{
				// A remote reference is verified by the existing peer protocol, never
				// guessed from a same-named local row. Distribution has no remote fetch.
				if node_id != node {
					return Err(Error::Forbidden);
				}
				refs.push((agent, "agent".into()));
			}
		}
		"bundle" => {
			let bundle: aidash_domain::registry::bindings::BundleConfig =
				serde_json::from_value(entry.config.clone())?;
			bundle.validate()?;
			if bundle
				.members
				.iter()
				.any(|reference| reference.registry_node != node)
			{
				return Err(Error::Forbidden);
			}
			refs.extend(
				bundle
					.members
					.into_iter()
					.map(|r| (r.local(), String::new())),
			);
		}
		"cluster" => {
			let c: ClusterConfig = serde_json::from_value(entry.config.clone())?;
			refs.push((c.coordinator, "agent".into()));
		}
		_ => {}
	}
	Ok(refs)
}
pub async fn local(scope: &mut dyn DefinitionScope, r: &EntityRef) -> Result<Entry> {
	let entry = scope.raw(r).await?;
	if let Some(projection) = &entry.installation {
		if projection.contract != 1 || projection.tenant != scope.tenant() {
			return Err(Error::Forbidden);
		}
		let install = scope
			.installation(&projection.installation)
			.await?
			.ok_or(Error::Forbidden)?;
		scope
			.require_installation_read(&install, projection.revision)
			.await?;
		let rev = scope.revision(&install.id, projection.revision).await?;
		if rev.entry != entry {
			return Err(Error::Forbidden);
		}
	} else {
		scope.catalog(r, "registry.read").await?;
	}
	Ok(entry)
}
/// Typed and explicit edges share a bounded traversal. A cycle is visited once;
/// no partial graph is returned if any referenced definition is unavailable.
pub async fn local_graph(
	scope: &mut dyn DefinitionScope,
	roots: Vec<(EntityRef, String)>,
	node: &str,
) -> Result<Vec<Entry>> {
	let mut queue = roots;
	let mut seen = BTreeSet::new();
	let mut entries = vec![];
	while let Some((r, kind)) = queue.pop() {
		let entry = local(scope, &r).await?;
		if !kind.is_empty() && entry.kind != kind {
			return Err(Error::Forbidden);
		}
		if !seen.insert((r.id, r.version)) {
			continue;
		}
		if seen.len() > 128 {
			return Err(Error::Forbidden);
		}
		queue.extend(refs(&entry, node).map_err(|_| Error::Forbidden)?);
		entries.push(entry);
	}
	Ok(entries)
}
pub async fn publication_graph(
	scope: &mut dyn DefinitionScope,
	root: &Entry,
	extra: &[EntityRef],
	node: &str,
) -> Result<Vec<Dependency>> {
	let mut queue = refs(root, node)?;
	queue.extend(extra.iter().cloned().map(|r| (r, String::new())));
	let mut seen = BTreeSet::new();
	let mut result = vec![];
	while let Some((r, kind)) = queue.pop() {
		let entry = scope.catalog(&r, "registry.read").await?;
		crate::registry::system::reject_distribution(&entry)?;
		scope.require_export(&entry).await?;
		if !kind.is_empty() && entry.kind != kind {
			return Err(Error::Forbidden);
		}
		if !seen.insert((r.id.clone(), r.version.clone())) {
			continue;
		}
		if seen.len() > 128 {
			return Err(Error::Forbidden);
		}
		queue.extend(refs(&entry, node)?);
		let package = if let Some(p) = &entry.installation {
			Some(
				scope
					.revision(&p.installation, p.revision)
					.await?
					.source
					.key,
			)
		} else {
			// A locally published exact dependency also supplies a disclosure
			// path before recipients install it. Names alone never bind content.
			let version = scope.matching_publication(&r, &content(&entry)).await?;
			version.and_then(|v| {
				if v.owner_tenant != scope.tenant() || v.source != r {
					return None;
				}
				manifest(&v)
					.is_ok_and(|p| content(&p.entity) == content(&entry))
					.then_some(v.key)
			})
		};
		result.push(Dependency {
			reference: r,
			kind: entry.kind.clone(),
			digest: content(&entry),
			package,
		});
	}
	result.sort_by_key(|d| (d.reference.id.clone(), d.reference.version.clone()));
	Ok(result)
}
pub async fn resolve(
	scope: &mut dyn DefinitionScope,
	validation: &DefinitionValidation,
	source: &Version,
	config: &Value,
	submitted: &[DependencyBinding],
	node: &str,
) -> Result<(Entry, Vec<EntityRef>, Vec<DependencyBinding>)> {
	let package = manifest(source)?;
	let mut entry = package.entity;
	// Configuration may tune local behavior, but executable references must
	// pass through the exact dependency bindings below (including tool kind).
	let protected: &[&str] = match entry.kind.as_str() {
		"agent" => &[
			"model",
			"bindings",
			"remove_default",
			"cluster",
			"schema_version",
		],
		"tool" if aidash_domain::tool::legacy_config(&entry.config)?.is_some() => {
			&["transport", "node_id", "agent"]
		}
		"tool" => &[
			"registry_node",
			"provider",
			"operation",
			"tier",
			"default_alias",
			"lifecycle",
		],
		"bundle" => &["members"],
		"cluster" => &["coordinator"],
		_ => &[],
	};
	if protected.iter().any(|field| config.get(*field).is_some()) {
		return Err(Error::Invalid(
			"dependency references require bindings, not configuration overrides".into(),
		));
	}
	if entry.kind == "tool"
		&& aidash_domain::tool::legacy_config(&entry.config)?.is_none()
		&& let Some(transport) = config.get("transport")
	{
		let original = entry.config.get("transport").ok_or(Error::Forbidden)?;
		// Transport parameters may be configured, while executor edges require
		// verified dependency substitution and the Provider kind stays pinned.
		if original.get("transport") != transport.get("transport")
			|| original.get("transport").and_then(Value::as_str) == Some("agent")
		{
			return Err(Error::Invalid(
				"executor references require dependency bindings".into(),
			));
		}
	}
	overlay_config(&mut entry.config, config)?;
	// Frozen bundle qualifiers name the publishing Node. Inspect them there;
	// only verified dependency substitutions become receiving-Node references.
	let direct = refs(
		&entry,
		if matches!(entry.kind.as_str(), "bundle" | "agent") {
			&source.repository
		} else {
			node
		},
	)?;
	let mut bindings = vec![];
	let mut seen = BTreeSet::new();
	for binding in submitted {
		if !seen.insert((binding.source.id.clone(), binding.source.version.clone())) {
			return Err(Error::Invalid("duplicate dependency binding".into()));
		}
		if !direct.iter().any(|(r, _)| *r == binding.source)
			|| !source
				.dependencies
				.iter()
				.any(|d| d.reference == binding.source)
		{
			return Err(Error::Forbidden);
		}
	}
	for dep in &source.dependencies {
		let target = if let Some(binding) = submitted.iter().find(|b| b.source == dep.reference) {
			binding.target.clone()
		} else if let Some(package) = &dep.package
			&& direct.iter().any(|(r, _)| *r == dep.reference)
		{
			let id = installation_id(scope.tenant(), package);
			let installed = scope.installation(&id).await?;
			if let Some(installed) = installed {
				reference(
					&scope
						.revision(
							&id,
							installed
								.active_revision
								.unwrap_or(installed.latest_revision),
						)
						.await?
						.entry,
				)
			} else {
				// Exact native content still qualifies; the checks below require
				// current catalog authority and the frozen digest, never a name alone.
				dep.reference.clone()
			}
		} else {
			dep.reference.clone()
		};
		let local = local(scope, &target).await?;
		if local.kind != dep.kind {
			return Err(Error::Forbidden);
		}
		if let Some(source_package) = &dep.package {
			if let Some(p) = &local.installation {
				if scope
					.revision(&p.installation, p.revision)
					.await?
					.source
					.key != *source_package
				{
					return Err(Error::Forbidden);
				}
			} else if content(&local) != dep.digest {
				return Err(Error::Forbidden);
			}
		} else if content(&local) != dep.digest {
			return Err(Error::Forbidden);
		}
		bindings.push(DependencyBinding {
			source: dep.reference.clone(),
			target,
		});
	}
	rewrite(&mut entry, &bindings, node)?;
	let mut roots = refs(&entry, node).map_err(|_| Error::Forbidden)?;
	roots.extend(bindings.iter().map(|b| (b.target.clone(), String::new())));
	let graph = local_graph(scope, roots, node).await?;
	// Structural validation never resolves node-local secret references.
	validation.validate_in(&entry, false)?;
	if entry.kind == "agent" {
		let snapshot = crate::registry::bindings::resolve(
			&mut Bindings { scope, node },
			validation,
			aidash_domain::registry::bindings::QualifiedRef {
				registry_node: node.into(),
				id: entry.id.clone(),
				version: entry.version.clone(),
			},
			&entry,
			false,
		)
		.await?;
		validation.bound_prompt_headroom(&snapshot, &Value::Null)?;
	}
	Ok((entry, graph.iter().map(reference).collect(), bindings))
}
pub async fn private_context(scope: &mut dyn DefinitionScope, entry: &Entry) -> Result<()> {
	if entry.kind != "agent" {
		return Ok(());
	}
	let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
	for binding in config.bindings.iter().filter(|b| {
		matches!(
			b.kind,
			aidash_domain::registry::bindings::BindingKind::Source
				| aidash_domain::registry::bindings::BindingKind::Memory
		)
	}) {
		let source = scope
			.catalog(&binding.target.local(), "registry.read")
			.await?;
		let context: aidash_domain::registry::bindings::sources::NativeContext =
			serde_json::from_value(source.config)?;
		context.validate(&source.kind)?;
		if let aidash_domain::registry::bindings::sources::NativeSource::ReferenceAttachments {
			references,
		} = context.source
		{
			for attachment in references {
				scope
					.require_reference_read(attachment.reference_id)
					.await?;
			}
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests;

struct Bindings<'a> {
	scope: &'a mut dyn DefinitionScope,
	node: &'a str,
}
#[async_trait::async_trait]
impl crate::ports::bindings::BindingCatalog for Bindings<'_> {
	async fn definition(
		&mut self,
		reference: &aidash_domain::registry::bindings::QualifiedRef,
	) -> Result<Entry> {
		if reference.registry_node != self.node {
			return Err(Error::Forbidden);
		}
		local(self.scope, &reference.local()).await
	}
	async fn installation(
		&mut self,
		projection: &aidash_domain::registry::Projection,
	) -> Result<()> {
		if projection.tenant != self.scope.tenant() || projection.contract != 1 {
			return Err(Error::Forbidden);
		}
		let installation = self
			.scope
			.installation(&projection.installation)
			.await?
			.ok_or(Error::Forbidden)?;
		if installation.active_revision != Some(projection.revision) {
			return Err(Error::Forbidden);
		}
		let revision = self
			.scope
			.revision(&installation.id, projection.revision)
			.await?;
		self.scope
			.catalog(&reference(&revision.entry), "registry.read")
			.await?;
		Ok(())
	}
	async fn source(&mut self, entry: &Entry) -> Result<()> {
		if entry.kind == "skill" {
			return Ok(());
		}
		let source: aidash_domain::registry::bindings::sources::NativeContext =
			serde_json::from_value(entry.config.clone())?;
		source.validate(&entry.kind)?;
		if let aidash_domain::registry::bindings::sources::NativeSource::ReferenceAttachments {
			references,
		} = source.source
		{
			for reference in references {
				self.scope
					.require_reference_read(reference.reference_id)
					.await?;
			}
		}
		Ok(())
	}
}
