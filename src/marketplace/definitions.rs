use super::{storage::*, *};
use crate::{
	Error, Result,
	authorization::{access::Access, catalog},
	registry::{AgentConfig, ClusterConfig},
};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use sqlx::{Postgres, Transaction};

pub(super) fn reference(entry: &Entry) -> EntityRef {
	EntityRef {
		id: entry.id.clone(),
		version: entry.version.clone(),
	}
}
pub(super) fn content(entry: &Entry) -> String {
	let mut entry = entry.clone();
	entry.id.clear();
	entry.version.clear();
	entry.installation = None;
	key(&entry)
}
pub(super) fn refs(entry: &Entry, node: &str) -> Result<Vec<(EntityRef, String)>> {
	let mut refs = vec![];
	match entry.kind.as_str() {
		"agent" => {
			let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
			refs.push((config.model, "model".into()));
			refs.extend(config.tools.into_iter().map(|r| (r, "tool".into())));
			refs.extend(config.skills.into_iter().map(|r| (r, "skill".into())));
			refs.extend(config.cluster.into_iter().map(|r| (r, "cluster".into())));
		}
		"tool" => {
			if let crate::tool::ToolConfig::Agent { node_id, agent } =
				serde_json::from_value(entry.config.clone())?
			{
				// A remote reference is verified by the existing peer protocol, never
				// guessed from a same-named local row. Distribution has no remote fetch.
				if node_id != node {
					return Err(Error::Forbidden);
				}
				refs.push((agent, "agent".into()));
			}
		}
		"cluster" => {
			let c: ClusterConfig = serde_json::from_value(entry.config.clone())?;
			refs.push((c.coordinator, "agent".into()));
		}
		_ => {}
	}
	Ok(refs)
}
pub(super) fn rewrite(entry: &mut Entry, bindings: &[DependencyBinding]) -> Result<()> {
	fn bind(reference: &mut EntityRef, bindings: &[DependencyBinding]) {
		if let Some(binding) = bindings.iter().find(|b| b.source == *reference) {
			*reference = binding.target.clone();
		}
	}
	match entry.kind.as_str() {
		"agent" => {
			let mut c: AgentConfig = serde_json::from_value(entry.config.clone())?;
			bind(&mut c.model, bindings);
			for r in c
				.tools
				.iter_mut()
				.chain(c.skills.iter_mut())
				.chain(c.cluster.iter_mut())
			{
				bind(r, bindings);
			}
			entry.config = serde_json::to_value(c)?;
		}
		"tool" => {
			let mut c: crate::tool::ToolConfig = serde_json::from_value(entry.config.clone())?;
			if let crate::tool::ToolConfig::Agent { agent, .. } = &mut c {
				bind(agent, bindings);
			}
			entry.config = serde_json::to_value(c)?;
		}
		"cluster" => {
			let mut c: ClusterConfig = serde_json::from_value(entry.config.clone())?;
			bind(&mut c.coordinator, bindings);
			entry.config = serde_json::to_value(c)?;
		}
		_ => {}
	}
	Ok(())
}
pub(super) async fn raw(tx: &mut Transaction<'_, Postgres>, r: &EntityRef) -> Result<Entry> {
	let value: Option<Value> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("metadata"))
			.from(Alias::new("registry"))
			.and_where(Expr::cust("id=$1 AND version=$2"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&r.id)
	.bind(&r.version)
	.fetch_optional(&mut **tx)
	.await?;
	serde_json::from_value(value.ok_or(Error::Forbidden)?).map_err(Into::into)
}
pub(super) async fn local(access: &mut Access, r: &EntityRef) -> Result<Entry> {
	let entry = raw(&mut access.tx, r).await?;
	if let Some(projection) = &entry.installation {
		if projection.contract != 1 || projection.tenant != access.identity.tenant {
			return Err(Error::Forbidden);
		}
		let install: Installation = get(
			&mut access.tx,
			"marketplace_installations",
			&projection.installation,
		)
		.await?
		.ok_or(Error::Forbidden)?;
		access
			.require(
				&super::installations::resource(access, &install, Some(projection.revision)),
				"installation.read",
			)
			.await?;
		let rev = super::installations::revision(&mut access.tx, &install.id, projection.revision)
			.await?;
		if rev.entry != entry {
			return Err(Error::Forbidden);
		}
	} else {
		catalog::entry(access, r, "registry.read").await?;
	}
	Ok(entry)
}
/// Typed and explicit edges share a bounded traversal. A cycle is visited once;
/// no partial graph is returned if any referenced definition is unavailable.
pub(super) async fn local_graph(
	access: &mut Access,
	roots: Vec<(EntityRef, String)>,
	node: &str,
) -> Result<Vec<Entry>> {
	let mut queue = roots;
	let mut seen = BTreeSet::new();
	let mut entries = vec![];
	while let Some((r, kind)) = queue.pop() {
		let entry = local(access, &r).await?;
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
pub(super) async fn publication_graph(
	access: &mut Access,
	root: &Entry,
	extra: &[EntityRef],
	node: &str,
) -> Result<Vec<Dependency>> {
	let mut queue = refs(root, node)?;
	queue.extend(extra.iter().cloned().map(|r| (r, String::new())));
	let mut seen = BTreeSet::new();
	let mut result = vec![];
	while let Some((r, kind)) = queue.pop() {
		let entry = catalog::entry(access, &r, "registry.read").await?;
		access
			.require(&catalog::resource(access, &entry), "registry.export")
			.await?;
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
				super::installations::revision(&mut access.tx, &p.installation, p.revision)
					.await?
					.source
					.key,
			)
		} else {
			// A locally published exact dependency also supplies a disclosure
			// path before recipients install it. Names alone never bind content.
			let versions: Vec<Version> = documents(&mut access.tx, "marketplace_versions").await?;
			versions
				.into_iter()
				.find(|v| {
					v.owner_tenant == access.identity.tenant
						&& v.source == r && super::distribution::manifest(v)
						.is_ok_and(|p| content(&p.entity) == content(&entry))
				})
				.map(|v| v.key)
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
pub(super) async fn resolve(
	access: &mut Access,
	source: &Version,
	config: &Value,
	submitted: &[DependencyBinding],
	node: &str,
) -> Result<(Entry, Vec<EntityRef>, Vec<DependencyBinding>)> {
	let package = super::distribution::manifest(source)?;
	let mut entry = package.entity;
	crate::registry::overlay_config(&mut entry.config, config)?;
	let mut bindings = vec![];
	let mut seen = BTreeSet::new();
	for binding in submitted {
		if !seen.insert((binding.source.id.clone(), binding.source.version.clone())) {
			return Err(Error::Invalid("duplicate dependency binding".into()));
		}
		if !source
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
		} else if let Some(package) = &dep.package {
			let id = super::installations::id(&access.identity.tenant, package);
			let installed: Option<Installation> =
				get(&mut access.tx, "marketplace_installations", &id).await?;
			if let Some(installed) = installed {
				reference(
					&super::installations::revision(
						&mut access.tx,
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
		let local = local(access, &target).await?;
		if local.kind != dep.kind {
			return Err(Error::Forbidden);
		}
		if let Some(source_package) = &dep.package {
			if let Some(p) = &local.installation {
				if super::installations::revision(&mut access.tx, &p.installation, p.revision)
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
	rewrite(&mut entry, &bindings)?;
	let mut roots = refs(&entry, node).map_err(|_| Error::Forbidden)?;
	roots.extend(bindings.iter().map(|b| (b.target.clone(), String::new())));
	let graph = local_graph(access, roots, node).await?;
	// Structural validation never resolves node-local secret references.
	crate::registry::validate_structure(&entry)?;
	if entry.kind == "agent" {
		crate::registry::validate_agent_prompt(
			&serde_json::from_value(entry.config.clone())?,
			&graph,
			&Value::Null,
		)?;
	}
	Ok((entry, graph.iter().map(reference).collect(), bindings))
}

pub(super) async fn private_context(access: &mut Access, entry: &Entry) -> Result<()> {
	if entry.kind != "agent" {
		return Ok(());
	}
	let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
	if config.knowledge_digest.is_some() && entry.installation.is_none() {
		// Node-local knowledge is not embedded in a package. Its source remains
		// a separate exact catalog dependency, even when the summary is shared.
		catalog::entry(access, &reference(entry), "registry.read").await?;
	}
	for attachment in config.reference_attachments {
		crate::capabilities::references::get(access, attachment.reference_id, "reference.read")
			.await?;
	}
	Ok(())
}
