//! Operator-only compatibility, activation and immutable recovery from legacy definitions.
use super::{definitions, installations};
use crate::{Error, Result, ports::marketplace::OperatorScope, registry::DefinitionValidation};
use aidash_domain::{
	marketplace::definitions::{content, key, manifest, prospective_installation, reference},
	marketplace::installations::{activation, compatible},
	marketplace::{Compatibility, Dependency, Installation, InstallationRevision, Version},
	registry::EntityRef,
};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use uuid::Uuid;

pub struct CompatibilityChange {
	pub enabled: bool,
	pub expected_revision: i64,
	pub compatible_instances_confirmed: bool,
}
pub struct ActivationCommand {
	pub tenant: String,
	pub revision: i64,
	pub expected_activation_revision: i64,
	pub expected_catalog_revision: i64,
	pub enabled: bool,
}
/// Preserve the legacy operation's exact serialized fingerprint field order.
#[derive(Serialize)]
pub struct AdoptionCommand {
	pub tenant: String,
	pub source: EntityRef,
	pub idempotency_key: Uuid,
}
pub struct AdministrationQuery {
	pub tenant: String,
	pub offset: usize,
	pub limit: usize,
}
pub struct AdministrationPage {
	pub entries: Vec<InstallationRevision>,
	pub next_offset: Option<usize>,
}

fn conflict() -> Error {
	Error::Conflict("revision or immutable content changed".into())
}
async fn gate(scope: &mut dyn OperatorScope) -> Result<()> {
	let state = scope.compatibility().await?.ok_or(Error::Forbidden)?;
	if !compatible(&state) {
		return Err(Error::Forbidden);
	}
	Ok(())
}
pub async fn compatibility(scope: &mut dyn OperatorScope) -> Result<Compatibility> {
	require_operator(scope.principal())?;
	scope.compatibility().await?.ok_or(Error::Forbidden)
}
pub fn validate_compatibility(input: &CompatibilityChange) -> Result<()> {
	if input.enabled && !input.compatible_instances_confirmed {
		return Err(Error::Invalid(
			"confirm every serving instance and worker supports Marketplace contract 1".into(),
		));
	}
	Ok(())
}
pub async fn set_compatibility(
	scope: &mut dyn OperatorScope,
	input: &CompatibilityChange,
) -> Result<Compatibility> {
	require_operator(scope.principal())?;
	validate_compatibility(input)?;
	scope.distribution_lock(true).await?;
	let current = scope.compatibility().await?.ok_or(Error::Forbidden)?;
	if input.expected_revision != current.revision {
		return Err(conflict());
	}
	let next = Compatibility {
		enabled: input.enabled,
		revision: current.revision + 1,
		contract: 1,
	};
	scope.save_compatibility(&next).await?;
	scope
		.event("marketplace.compatibility_changed", json!(next))
		.await?;
	Ok(next)
}
pub async fn activate(
	scope: &mut dyn OperatorScope,
	id: &str,
	input: &ActivationCommand,
) -> Result<Installation> {
	require_operator(scope.principal())?;
	scope.load_tenant(&input.tenant).await?;
	scope.distribution_lock(true).await?;
	if input.enabled {
		gate(scope).await?;
	}
	scope.enable_writer().await?;
	let install = scope.installation(id).await?.ok_or(Error::Forbidden)?;
	if install.tenant != input.tenant {
		return Err(Error::Forbidden);
	}
	if install.activation_revision != input.expected_activation_revision {
		return Err(conflict());
	}
	let revision = scope.revision(id, input.revision).await?;
	if input.enabled {
		for dependency in &revision.dependencies {
			if !scope.approved(&input.tenant, dependency).await? {
				return Err(Error::Forbidden);
			}
		}
	}
	scope
		.set_approval(
			&input.tenant,
			&reference(&revision.entry),
			input.expected_catalog_revision,
			input.enabled,
		)
		.await?;
	let install = activation(install, input.revision, input.enabled);
	scope.save_installation(&install).await?;
	scope.event("marketplace.activation_changed", json!({"installation":id,"tenant":input.tenant,"revision":input.revision,"enabled":input.enabled,"actor":"operator"})).await?;
	Ok(install)
}
pub async fn adopt(
	scope: &mut dyn OperatorScope,
	validation: &DefinitionValidation,
	input: &AdoptionCommand,
	node: &str,
) -> Result<Installation> {
	require_operator(scope.principal())?;
	aidash_domain::policy::identifier(&input.tenant)?;
	scope.load_tenant(&input.tenant).await?;
	scope.distribution_lock(true).await?;
	gate(scope).await?;
	scope.enable_writer().await?;
	let replay_key = key(&("operator-adoption", &input.tenant, input.idempotency_key));
	let fingerprint = key(input);
	if let Some(saved) = scope.adoption_replay(&replay_key).await? {
		if saved["fingerprint"] != fingerprint {
			return Err(conflict());
		}
		return scope
			.installation(saved["id"].as_str().ok_or(Error::Forbidden)?)
			.await?
			.ok_or(Error::Forbidden);
	}
	let raw = scope.raw_definition(&input.source).await?;
	if raw.installation.is_some() {
		return Err(Error::Invalid("adoption requires a legacy source".into()));
	}
	let effective = scope.effective_legacy(&input.source).await?;
	let (manifest_source, digest) = scope
		.package_record(&input.source)
		.await?
		.ok_or(Error::Forbidden)?;
	let mut package: aidash_domain::registry::Package = serde_json::from_str(&manifest_source)?;
	if package.entity != raw
		|| format!("sha256:{:x}", Sha256::digest(manifest_source.as_bytes())) != digest
	{
		return Err(conflict());
	}
	// Freeze the executable overlay; transitive overlays cannot be copied as a root revision.
	let mut queue = definitions::refs(&effective, node)?;
	let mut seen = BTreeSet::new();
	let mut dependencies = vec![];
	while let Some((reference, kind)) = queue.pop() {
		let entry = scope.raw_definition(&reference).await?;
		let effective_dependency = scope.effective_legacy(&reference).await?;
		let effective_refs = definitions::refs(&effective_dependency, node)?;
		if entry != effective_dependency || (!kind.is_empty() && entry.kind != kind) {
			return Err(Error::Forbidden);
		}
		if !seen.insert((reference.id.clone(), reference.version.clone())) {
			continue;
		}
		if seen.len() > 128
			|| entry
				.installation
				.as_ref()
				.is_some_and(|p| p.tenant != input.tenant)
			|| !scope.approved(&input.tenant, &reference).await?
		{
			return Err(Error::Forbidden);
		}
		queue.extend(effective_refs);
		dependencies.push(Dependency {
			reference,
			kind: entry.kind.clone(),
			digest: content(&entry),
			package: None,
		});
	}
	package.entity = effective.clone();
	package.dependencies = dependencies.iter().map(|d| d.reference.clone()).collect();
	let manifest_source = serde_json::to_string(&package)?;
	if manifest_source.len() > 1_048_576 {
		return Err(Error::Invalid("package exceeds one MiB".into()));
	}
	let source_key = key(&(node, &input.tenant, &raw.id, &raw.version));
	let source = Version {
		key: source_key.clone(),
		repository: node.into(),
		owner_tenant: input.tenant.clone(),
		package_id: raw.id.clone(),
		version: raw.version.clone(),
		kind: raw.kind.clone(),
		publisher: "operator-adoption".into(),
		source: input.source.clone(),
		digest: format!("sha256:{:x}", Sha256::digest(manifest_source.as_bytes())),
		manifest_source,
		dependencies,
		lineage: scope.provenance(&raw, &input.tenant).await?,
	};
	manifest(&source)?;
	// Imported provenance cannot disappear through operator adoption.
	if !source.lineage.is_empty() {
		return Err(Error::Forbidden);
	}
	if let Some(existing) = scope.version(&source_key).await? {
		if existing.manifest_source != source.manifest_source
			|| existing.publisher != "operator-adoption"
		{
			return Err(conflict());
		}
	} else {
		scope
			.insert_version(
				&source,
				&aidash_domain::marketplace::publication::initial_audience(&source),
			)
			.await?;
	}
	let install = prospective_installation(&input.tenant, &source_key);
	if scope.installation(&install.id).await?.is_none() {
		let dependencies = source
			.dependencies
			.iter()
			.map(|d| d.reference.clone())
			.collect();
		let config = scope
			.legacy_config(&input.source)
			.await?
			.unwrap_or_else(|| json!({}));
		installations::stage(
			scope,
			validation,
			install.clone(),
			&source,
			config,
			(effective, dependencies, vec![]),
		)
		.await?;
	}
	scope
		.remember_adoption(
			&replay_key,
			json!({"fingerprint":fingerprint,"id":install.id}),
		)
		.await?;
	scope
		.installation(&install.id)
		.await?
		.ok_or(Error::Forbidden)
}
pub async fn administration(
	scope: &mut dyn OperatorScope,
	input: &AdministrationQuery,
) -> Result<AdministrationPage> {
	require_operator(scope.principal())?;
	aidash_domain::policy::identifier(&input.tenant)?;
	scope.distribution_lock(false).await?;
	let mut rows = scope
		.revision_documents(&input.tenant, input.offset, (input.limit + 1) as u64)
		.await?;
	let more = rows.len() > input.limit;
	rows.truncate(input.limit);
	let mut entries = vec![];
	for (installation, revision) in rows {
		let installation: Installation = serde_json::from_value(installation)?;
		let revision: aidash_domain::marketplace::Revision = serde_json::from_value(revision)?;
		let approved = scope
			.approved(&input.tenant, &reference(&revision.entry))
			.await?;
		entries.push(InstallationRevision {
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
	Ok(AdministrationPage {
		entries,
		next_offset: more.then_some(input.offset + input.limit),
	})
}
#[cfg(test)]
mod tests;

pub use crate::authorization::require_operator;
