//! Edit the restriction of an installation's active revision without
//! activating anything. Activation stays a separate decision, so a staged
//! newer revision never blocks a restriction edit on the active one.
use super::*;
use aidash_domain::{identity::catalog::CatalogRestriction, registry::rules::digest};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RestrictionSelection {
	pub installation: String,
	/// The active revision whose Catalog Approval the edit applies to.
	pub revision: i64,
	pub digest: String,
	/// Fenced, never bumped: a restriction edit does not stale a pending
	/// activation review of the same installation.
	pub expected_activation_revision: i64,
	pub expected_catalog_revision: i64,
	/// Replaces the stored restriction; empty clears it.
	pub restriction: CatalogRestriction,
}
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RestrictionSet {
	pub tenant: String,
	/// The Node the approvals apply to; absent keeps the default Node.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub node: Option<aidash_domain::nodes::NodeName>,
	pub installations: Vec<RestrictionSelection>,
}

/// The native caller owns a transaction; any stale fence rejects the whole set
/// before a write. Installation pointers and activation revisions are untouched.
pub async fn restrict_active(
	scope: &mut dyn DeploymentAdministratorScope,
	input: &RestrictionSet,
) -> Result<()> {
	aidash_domain::policy::identifier(&input.tenant)?;
	super::require_approving(scope, &input.tenant)?;
	if input.installations.is_empty() || input.installations.len() > 32 {
		return Err(Error::Invalid(
			"restriction set exceeds bounds or is empty".into(),
		));
	}
	scope.load_tenant(&input.tenant).await?;
	scope.distribution_lock(true).await?;
	scope.enable_writer().await?;
	let mut ids = BTreeSet::new();
	let mut writes = vec![];
	for selection in &input.installations {
		if !ids.insert(&selection.installation) || selection.revision < 1 {
			return Err(Error::Invalid(
				"duplicate installation or invalid revision in restriction set".into(),
			));
		}
		aidash_domain::identity::catalog::validate_revision(selection.expected_catalog_revision)?;
		selection.restriction.validate()?;
		let install = scope
			.installation(&selection.installation)
			.await?
			.ok_or(Error::Forbidden)?;
		if install.tenant != input.tenant {
			return Err(Error::Forbidden);
		}
		if install.active_revision != Some(selection.revision)
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
		{
			return Err(conflict());
		}
		let entry = reference(&revision.entry);
		// Only an approved revision has a restriction to edit; approving one
		// is activation's decision.
		if !scope.approved(&input.tenant, &entry).await? {
			return Err(conflict());
		}
		if scope.catalog_revision(&input.tenant, &entry).await?
			!= selection.expected_catalog_revision
		{
			return Err(conflict());
		}
		writes.push((entry, selection));
	}
	for (entry, selection) in writes {
		scope
			.set_approval(
				&input.tenant,
				&entry,
				selection.expected_catalog_revision,
				true,
				crate::authorization::catalog::restriction_change(Some(
					selection.restriction.clone(),
				)),
			)
			.await?;
	}
	scope.event("marketplace.restriction_updated",json!({"tenant":input.tenant,"selection_digest":digest(&serde_json::to_value(input)?),"installations":input.installations.iter().map(|selection| json!({"installation":selection.installation,"revision":selection.revision})).collect::<Vec<_>>(),"actor":actor(scope.principal())})).await?;
	Ok(())
}

#[cfg(test)]
mod tests;
