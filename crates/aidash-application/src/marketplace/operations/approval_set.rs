//! Review an exact pending closure, then approve and activate it in one scope.
use super::*;
use aidash_domain::registry::rules::digest;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PendingSelection {
	pub installation: String,
	pub revision: i64,
	pub digest: String,
	pub expected_activation_revision: i64,
}
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalSelection {
	pub reference: EntityRef,
	pub expected_catalog_revision: i64,
}
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalSet {
	pub tenant: String,
	pub installations: Vec<PendingSelection>,
	pub approvals: Vec<ApprovalSelection>,
}

/// The native caller owns a transaction; failures roll back approvals, active
/// pointers and events together. No member is discovered after writes begin.
pub async fn approve_and_activate(
	scope: &mut dyn OperatorScope,
	validation: &DefinitionValidation,
	input: &ApprovalSet,
	node: &str,
) -> Result<Vec<Installation>> {
	require_operator(scope.principal())?;
	aidash_domain::configuration::validate_node_id(node)?;
	aidash_domain::policy::identifier(&input.tenant)?;
	if input.installations.is_empty()
		|| input.installations.len() > 32
		|| input.approvals.is_empty()
		|| input.approvals.len() > 128
	{
		return Err(Error::Invalid(
			"approval set exceeds bounds or is empty".into(),
		));
	}
	scope.load_tenant(&input.tenant).await?;
	scope.distribution_lock(true).await?;
	gate(scope).await?;
	scope.enable_writer().await?;
	let mut installs = vec![];
	let mut references = std::collections::BTreeMap::new();
	let mut dependencies = BTreeSet::new();
	let mut ids = BTreeSet::new();
	for selection in &input.installations {
		if !ids.insert(&selection.installation) || selection.revision < 1 {
			return Err(Error::Invalid(
				"duplicate installation or invalid revision in approval set".into(),
			));
		}
		let install = scope
			.installation(&selection.installation)
			.await?
			.ok_or(Error::Forbidden)?;
		if install.tenant != input.tenant {
			return Err(Error::Forbidden);
		}
		if install.latest_revision != selection.revision
			|| install.activation_revision != selection.expected_activation_revision
		{
			return Err(conflict());
		}
		let revision = scope.revision(&install.id, selection.revision).await?;
		if revision.digest != selection.digest
			|| revision.digest != key(&revision.entry)
			|| revision.revision != selection.revision
			|| revision.tenant != input.tenant
			|| revision.installation != install.id
			|| revision.source.key != install.package_key
		{
			return Err(conflict());
		}
		crate::registry::system::reject_distribution(&revision.entry)?;
		let mut admission = revision.entry.clone();
		admission.installation = None;
		validation.validate_in(&admission, true)?;
		let entry = reference(&revision.entry);
		let projection = revision
			.entry
			.installation
			.as_ref()
			.ok_or(Error::Forbidden)?;
		if projection.tenant != input.tenant
			|| projection.installation != install.id
			|| projection.revision != selection.revision
		{
			return Err(Error::Forbidden);
		}
		if references
			.insert((entry.id, entry.version), revision.entry)
			.is_some()
		{
			return Err(Error::Invalid(
				"multiple installations selected the same definition".into(),
			));
		}
		dependencies.extend(
			revision
				.dependencies
				.into_iter()
				.map(|reference| (reference.id, reference.version)),
		);
		installs.push(activation(install, selection.revision, true));
	}
	let mut requested = BTreeSet::new();
	for approval in &input.approvals {
		let key = (
			approval.reference.id.clone(),
			approval.reference.version.clone(),
		);
		if !requested.insert(key.clone()) || !references.contains_key(&key) {
			return Err(Error::Invalid(
				"approval set contains an unreviewed or duplicate definition".into(),
			));
		}
		aidash_domain::identity::catalog::validate_revision(approval.expected_catalog_revision)?;
		if scope
			.catalog_revision(&input.tenant, &approval.reference)
			.await? != approval.expected_catalog_revision
		{
			return Err(conflict());
		}
	}
	if requested != references.keys().cloned().collect() {
		return Err(Error::Invalid(
			"approval set must enumerate every selected installation revision".into(),
		));
	}
	let mut admitted = references.clone();
	for (id, version) in dependencies {
		if requested.contains(&(id.clone(), version.clone())) {
			continue;
		}
		if !scope
			.approved(
				&input.tenant,
				&EntityRef {
					id: id.clone(),
					version: version.clone(),
				},
			)
			.await?
		{
			return Err(Error::Forbidden);
		}
		let entry = scope
			.raw_definition(&EntityRef {
				id: id.clone(),
				version: version.clone(),
			})
			.await?;
		if entry.id != id || entry.version != version {
			return Err(Error::Forbidden);
		}
		if let Some(projection) = &entry.installation {
			let installation = scope
				.installation(&projection.installation)
				.await?
				.ok_or(Error::Forbidden)?;
			let retained = scope
				.revision(&projection.installation, projection.revision)
				.await?;
			if projection.tenant != input.tenant
				|| installation.tenant != input.tenant
				|| installation.active_revision != Some(projection.revision)
				|| retained.entry != entry
			{
				return Err(Error::Forbidden);
			}
		}
		let mut local = entry.clone();
		local.installation = None;
		validation.validate_in(&local, true)?;
		admitted.insert((id, version), entry);
	}
	// Recheck the exact immutable closure rather than trusting a cached dependency
	// list. This never adds an approval to the user's reviewed selection.
	for entry in admitted.values() {
		for (reference, kind) in definitions::refs(entry, node)? {
			let dependency = admitted
				.get(&(reference.id, reference.version))
				.ok_or(Error::Forbidden)?;
			if !kind.is_empty() && dependency.kind != kind {
				return Err(Error::Forbidden);
			}
			if entry.kind == "bundle" && !matches!(dependency.kind.as_str(), "tool" | "bundle") {
				return Err(Error::Forbidden);
			}
		}
		if entry.kind == "tool" {
			let descriptor: aidash_domain::tool::providers::ToolDescriptor =
				serde_json::from_value(entry.config.clone())?;
			if descriptor.registry_node != node {
				return Err(Error::Forbidden);
			}
			if let Some(lifecycle) = descriptor.lifecycle {
				for (reference, suffix) in [(lifecycle.poll, "poll"), (lifecycle.cancel, "cancel")]
				{
					let companion = admitted
						.get(&(reference.id, reference.version))
						.ok_or(Error::Forbidden)?;
					let actual: aidash_domain::tool::providers::ToolDescriptor =
						serde_json::from_value(companion.config.clone())?;
					let prefix = if descriptor.operation == "shell" {
						"shell"
					} else {
						"python"
					};
					if actual.provider != descriptor.provider
						|| actual.operation != format!("{prefix}_{suffix}")
						|| actual.lifecycle.is_some()
					{
						return Err(Error::Forbidden);
					}
				}
			}
		}
	}
	let graph = admitted
		.values()
		.map(|entry| {
			(
				aidash_domain::registry::bindings::QualifiedRef {
					registry_node: node.into(),
					id: entry.id.clone(),
					version: entry.version.clone(),
				},
				entry.clone(),
			)
		})
		.collect();
	crate::registry::bindings::validate_bundle_graph(&graph)?;
	for approval in &input.approvals {
		scope
			.set_approval(
				&input.tenant,
				&approval.reference,
				approval.expected_catalog_revision,
				true,
			)
			.await?;
	}
	for install in &installs {
		scope.save_installation(install).await?;
	}
	scope.event("marketplace.approval_set_activated",json!({"tenant":input.tenant,"selection_digest":digest(&serde_json::to_value(input)?),"installations":installs})).await?;
	Ok(installs)
}

#[cfg(test)]
mod tests;
