//! Authenticated operator declarations and complete pending installation groups.
use super::*;
use crate::ports::bindings::ProviderCatalog;
use aidash_domain::{
	marketplace::installations::stage_revision,
	registry::{Entry, Package},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostPackages {
	pub tenant: String,
	pub groups: Vec<String>,
	pub idempotency_key: Uuid,
}
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PendingHostPackages {
	pub installations: Vec<InstallationRevision>,
	pub unavailable: BTreeMap<String, String>,
}

fn version(
	tenant: &str,
	node: &str,
	entry: &Entry,
	entries: &BTreeMap<EntityKey, Entry>,
) -> Result<Version> {
	let direct = definitions::refs(entry, node)?;
	let mut pending = direct.clone();
	let mut seen = BTreeSet::new();
	let mut dependencies = vec![];
	while let Some((reference, kind)) = pending.pop() {
		if !seen.insert((reference.id.clone(), reference.version.clone())) {
			continue;
		}
		let dependency = entries
			.get(&(reference.id.clone(), reference.version.clone()))
			.ok_or(Error::Forbidden)?;
		if !kind.is_empty() && dependency.kind != kind {
			return Err(Error::Forbidden);
		}
		pending.extend(definitions::refs(dependency, node)?);
		dependencies.push(Dependency {
			reference: reference.clone(),
			kind: dependency.kind.clone(),
			digest: content(dependency),
			package: Some(source_key(tenant, node, dependency)),
		});
	}
	dependencies.sort_by_key(|d| (d.reference.id.clone(), d.reference.version.clone()));
	let manifest_source = serde_json::to_string(&Package {
		entity: entry.clone(),
		author: "Aidash Node operator".into(),
		permissions: vec!["tool.invoke".into()],
		dependencies: direct.into_iter().map(|(r, _)| r).collect(),
	})?;
	Ok(Version {
		key: source_key(tenant, node, entry),
		repository: node.into(),
		owner_tenant: tenant.into(),
		package_id: entry.id.clone(),
		version: entry.version.clone(),
		kind: entry.kind.clone(),
		publisher: "node-operator".into(),
		source: reference(entry),
		digest: format!("sha256:{:x}", Sha256::digest(manifest_source.as_bytes())),
		manifest_source,
		dependencies,
		lineage: BTreeSet::new(),
	})
}
type EntityKey = (String, String);
fn source_key(tenant: &str, node: &str, entry: &Entry) -> String {
	key(&(
		"node-host-contract-1",
		node,
		tenant,
		&entry.id,
		&entry.version,
	))
}

/// All providers, source bytes and dependency substitutions are checked before
/// writes. The caller retains one operator/distribution transaction for the full
/// set, including source versions, projections, provenance, replay and outbox.
/// This explicit operation never approves a catalog entry or active pointer.
pub async fn provision(
	scope: &mut dyn OperatorScope,
	validation: &DefinitionValidation,
	providers: &dyn ProviderCatalog,
	input: &HostPackages,
	node: &str,
) -> Result<PendingHostPackages> {
	require_operator(scope.principal())?;
	aidash_domain::policy::identifier(&input.tenant)?;
	if input.groups.is_empty() || input.groups.len() > 6 {
		return Err(Error::Invalid(
			"select one to six host package groups".into(),
		));
	}
	scope.load_tenant(&input.tenant).await?;
	scope.distribution_lock(true).await?;
	gate(scope).await?;
	scope.enable_writer().await?;
	let replay_key = key(&(
		"host-package-provision",
		&input.tenant,
		input.idempotency_key,
	));
	let fingerprint = key(input);
	// Current provider availability is checked even when serving a receipt.
	let plan = crate::registry::system::packages::configured_defaults(
		scope.principal(),
		validation,
		providers,
		node,
		&input.groups,
	)?;
	if let Some(saved) = scope.adoption_replay(&replay_key).await? {
		if saved["fingerprint"] != fingerprint {
			return Err(conflict());
		}
		return serde_json::from_value(saved["result"].clone()).map_err(Into::into);
	}
	let entries: BTreeMap<EntityKey, Entry> = plan
		.pending
		.iter()
		.flat_map(|group| {
			group
				.operations
				.iter()
				.chain(std::iter::once(&group.bundle))
		})
		.map(|entry| ((entry.id.clone(), entry.version.clone()), entry.clone()))
		.collect();
	let mut sources = BTreeMap::new();
	let mut projected = BTreeMap::new();
	let mut existing = BTreeMap::new();
	let mut ids = BTreeMap::new();
	for (identity, entry) in &entries {
		validation.validate_in(entry, true)?;
		let source = version(&input.tenant, node, entry, &entries)?;
		manifest(&source)?;
		if let Some(saved) = scope.version(&source.key).await? {
			if key(&saved) != key(&source) {
				return Err(conflict());
			}
		}
		let installation = prospective_installation(&input.tenant, &source.key);
		if let Some(saved) = scope.installation(&installation.id).await? {
			if saved.tenant != input.tenant || saved.package_key != source.key {
				return Err(Error::Forbidden);
			}
			let revision = scope.revision(&saved.id, saved.latest_revision).await?;
			if key(&revision.source) != key(&source)
				|| revision.config != json!({})
				|| revision.digest != key(&revision.entry)
			{
				return Err(conflict());
			}
			projected.insert(identity.clone(), reference(&revision.entry));
			existing.insert(identity.clone(), (saved, revision));
		} else {
			let id = scope.allocate_entry_id();
			projected.insert(
				identity.clone(),
				EntityRef {
					id: format!("mkt-{}", id.simple()),
					version: "1.0.0".into(),
				},
			);
			ids.insert(identity.clone(), id);
		}
		sources.insert(identity.clone(), source);
	}
	let substitutions: Vec<_> = projected
		.iter()
		.map(
			|((id, version), target)| aidash_domain::marketplace::DependencyBinding {
				source: EntityRef {
					id: id.clone(),
					version: version.clone(),
				},
				target: target.clone(),
			},
		)
		.collect();
	let mut staged = BTreeMap::new();
	for (identity, original) in &entries {
		let source = &sources[identity];
		let mut entry = original.clone();
		aidash_domain::marketplace::definitions::rewrite(&mut entry, &substitutions)?;
		let dependencies = source
			.dependencies
			.iter()
			.map(|d| projected[&(d.reference.id.clone(), d.reference.version.clone())].clone())
			.collect::<Vec<_>>();
		let bindings = substitutions
			.iter()
			.filter(|b| source.dependencies.iter().any(|d| d.reference == b.source))
			.cloned()
			.collect::<Vec<_>>();
		if let Some((_, revision)) = existing.get(identity) {
			if content(&revision.entry) != content(&entry)
				|| revision.dependencies != dependencies
				|| key(&revision.bindings) != key(&bindings)
			{
				return Err(conflict());
			}
			continue;
		}
		let candidate = stage_revision(
			prospective_installation(&input.tenant, &source.key),
			source,
			json!({}),
			(entry, dependencies, bindings),
			ids[identity],
		);
		validation.validate_in(&candidate.revision.entry, false)?;
		staged.insert(identity.clone(), candidate);
	}
	// Poll/cancel leaves are persisted before starts, and bundles last.
	let mut order = entries.keys().cloned().collect::<Vec<_>>();
	order.sort_by_key(|identity| {
		let entry = &entries[identity];
		if entry.kind == "bundle" {
			2
		} else if entry.config.get("lifecycle").is_some_and(|v| !v.is_null()) {
			1
		} else {
			0
		}
	});
	for identity in &order {
		let source = &sources[identity];
		if scope.version(&source.key).await?.is_none() {
			scope
				.insert_version(
					source,
					&aidash_domain::marketplace::publication::initial_audience(source),
				)
				.await?;
		}
		if let Some(candidate) = staged.get(identity) {
			scope
				.persist_revision(&candidate.installation, &candidate.revision)
				.await?;
			scope
				.save_provenance(&candidate.revision.entry, &candidate.provenance)
				.await?;
			scope
				.save_copy_provenance(
					&candidate.revision.entry,
					&input.tenant,
					&candidate.provenance,
				)
				.await?;
		}
	}
	let mut result = PendingHostPackages {
		installations: vec![],
		unavailable: plan.unavailable,
	};
	for identity in order {
		let (installation, revision) = if let Some(candidate) = staged.remove(&identity) {
			(candidate.installation, candidate.revision)
		} else {
			existing.remove(&identity).ok_or(Error::Forbidden)?
		};
		let approved = scope
			.approved(&input.tenant, &reference(&revision.entry))
			.await?;
		result.installations.push(InstallationRevision {
			installation,
			revision: revision.revision,
			entry: revision.entry,
			digest: revision.digest,
			config: revision.config,
			dependencies: revision.dependencies,
			bindings: revision.bindings,
			approved,
			actions: vec![],
		});
	}
	scope.event("marketplace.host_packages_pending",json!({"tenant":input.tenant,"groups":input.groups,"unavailable":result.unavailable,"installations":result.installations.iter().map(|r| &r.installation.id).collect::<Vec<_>>()})).await?;
	scope
		.remember_adoption(
			&replay_key,
			json!({"fingerprint":fingerprint,"result":result}),
		)
		.await?;
	Ok(result)
}

#[cfg(test)]
mod tests;
