//! Exact definition admission and discovery share current policy and inherited approval fences.
use crate::{Error, Result, ports::catalog::CatalogScope};
use aidash_domain::registry::{EntityRef, Entry, Search};
pub async fn entry(
	scope: &mut dyn CatalogScope,
	reference: &EntityRef,
	action: &str,
) -> Result<Entry> {
	if scope.inherited_lease() && !scope.approved(reference) {
		return Err(Error::Forbidden);
	}
	if !scope.inherited_lease() {
		scope.distribution_lock().await?;
	}
	let document = scope.document(reference).await?.ok_or(Error::Forbidden)?;
	let entry: Entry = serde_json::from_value(document)?;
	if let Some(projection) = &entry.installation
		&& (projection.contract != 1 || projection.tenant != scope.tenant())
	{
		return Err(Error::Forbidden);
	}
	scope.require(&scope.resource(&entry), action).await?;
	scope.remember(reference);
	Ok(entry)
}
pub async fn list(scope: &mut dyn CatalogScope, search: &Search) -> Result<Vec<Entry>> {
	if !scope.inherited_lease() {
		scope.distribution_lock().await?;
	}
	let documents = scope.documents().await?;
	let mut entries = vec![];
	for document in documents {
		let entry: Entry = serde_json::from_value(document)?;
		let reference = EntityRef {
			id: entry.id.clone(),
			version: entry.version.clone(),
		};
		if (!scope.inherited_lease() || scope.approved(&reference))
			&& scope.active(&entry).await?
			&& search.matches(&entry)
			&& scope
				.decide(&scope.resource(&entry), "registry.read")
				.await?
		{
			scope.remember(&reference);
			match scope.check_pinned(&entry).await {
				Ok(()) => entries.push(entry),
				Err(Error::Forbidden) => {}
				Err(error) => return Err(error),
			}
		}
	}
	Ok(entries)
}
pub async fn get(scope: &mut dyn CatalogScope, reference: &EntityRef) -> Result<Entry> {
	let entry = entry(scope, reference, "registry.read").await?;
	scope.check_pinned(&entry).await?;
	Ok(entry)
}
#[cfg(test)]
mod tests;

/// Shared with Marketplace activation, whose caller already holds its writer and admission lease.
/// All effects and the resulting history remain in the caller's transaction.
pub async fn set_in(
	scope: &mut dyn crate::ports::catalog::CatalogMutation,
	tenant: &str,
	reference: &EntityRef,
	expected_revision: i64,
	enabled: bool,
	actor: &str,
) -> Result<aidash_domain::identity::catalog::Binding> {
	aidash_domain::identity::catalog::validate_revision(expected_revision)?;
	if !scope.registered(reference).await? {
		return Err(Error::NotFound("registry entry".into()));
	}
	let binding = scope
		.compare_and_set(tenant, reference, expected_revision, enabled)
		.await?
		.ok_or_else(|| Error::Conflict("catalog revision changed".into()))?;
	scope.history(&binding, actor).await?;
	Ok(binding)
}

pub async fn set_catalog(
	scope: &mut dyn crate::ports::catalog::CatalogAdministrator,
	tenant: &str,
	reference: &EntityRef,
	expected_revision: i64,
	enabled: bool,
	actor: &str,
) -> Result<aidash_domain::identity::catalog::Binding> {
	super::require_operator(scope.principal())?;
	aidash_domain::identity::catalog::validate_revision(expected_revision)?;
	scope.load_tenant(tenant).await?;
	let definition = scope.definition(reference).await?;
	if !aidash_domain::marketplace::installations::catalog_update_allowed(&definition) {
		return Err(Error::Forbidden);
	}
	set_in(scope, tenant, reference, expected_revision, enabled, actor).await
}

pub async fn administration(
	scope: &mut dyn crate::ports::catalog::CatalogAdministrationRead,
	tenant: &str,
) -> Result<Vec<aidash_domain::identity::catalog::Binding>> {
	super::require_operator(scope.principal())?;
	scope.bindings(tenant).await
}

#[cfg(test)]
mod mutation_tests;

pub mod retained;

pub fn attributes(entry: &Entry) -> serde_json::Value {
	serde_json::json!({"version":entry.version,"capabilities":entry.capabilities,"tags":entry.tags,"languages":entry.languages,"config":entry.config})
}
