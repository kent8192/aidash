//! Exact definition admission and discovery share current policy and inherited approval fences.
use crate::{Error, Result, ports::catalog::CatalogScope};
use aidash_domain::{
	identity::catalog::{CatalogRestriction, RestrictionChange},
	registry::{EntityRef, Entry, Search},
};
pub async fn entry(
	scope: &mut dyn CatalogScope,
	reference: &EntityRef,
	action: &str,
) -> Result<Entry> {
	admit(scope, reference, action, None, &[]).await
}

/// [`entry`] for a definition pinned in an executing agent's Binding closure.
/// `users` are the closure agents that use it directly
/// ([`BindingSnapshot::delegated_users`]); empty when the executing agent uses
/// it itself. A definition only delegated agents use is pinned as metadata by
/// the executing agent: its restriction must admit those agents, who are
/// checked again as the direct user when their own delegated Runs start.
///
/// [`BindingSnapshot::delegated_users`]: aidash_domain::registry::bindings::BindingSnapshot::delegated_users
pub async fn closure_entry(
	scope: &mut dyn CatalogScope,
	reference: &EntityRef,
	action: &str,
	users: &[EntityRef],
) -> Result<Entry> {
	admit(scope, reference, action, None, users).await
}

/// The refusal of a Tenant policy denial met while admitting a Catalog entry for `tool`'s call.
pub fn policy_refusal(tool: &str, action: &str) -> Error {
	Error::AuthorityRefused(Box::new(aidash_domain::tool::AuthorityRefusal {
		tool: tool.to_owned(),
		check: aidash_domain::tool::RefusalCheck::Policy,
		action: Some(action.to_owned()),
	}))
}

/// [`entry`] for the call of `tool`: a policy denial is [`policy_refusal`], so it is not
/// reported as a Catalog Approval refusal.
pub async fn tool_entry(
	scope: &mut dyn CatalogScope,
	reference: &EntityRef,
	action: &str,
	tool: &str,
) -> Result<Entry> {
	admit(scope, reference, action, Some(tool), &[]).await
}

async fn admit(
	scope: &mut dyn CatalogScope,
	reference: &EntityRef,
	action: &str,
	tool: Option<&str>,
	users: &[EntityRef],
) -> Result<Entry> {
	if scope.inherited_lease()
		&& !scope.approved(reference)
		&& !crate::registry::system::builtin_reference(reference)
	{
		return Err(Error::Forbidden);
	}
	// A cache hit already holds the distribution lock taken by its first
	// admission in this transaction; a new admission takes it before reading
	// the restriction so a concurrent change cannot slip in between.
	if !scope.inherited_lease() && scope.admitted(reference, action).is_none() {
		scope.distribution_lock().await?;
	}
	// The restriction is checked on every use, including a cached or inherited
	// admission: an agent's approved closure does not make a dependency usable
	// by another agent that a delegated Run adds to the subject chain.
	let delegated = users
		.iter()
		.map(|agent| scope.agent_subject(agent))
		.collect::<Option<Vec<_>>>()
		.filter(|subjects| !subjects.is_empty());
	let direct_users = delegated.unwrap_or_else(|| vec![scope.direct_user(reference)]);
	for direct_user in &direct_users {
		if !scope.permits_restriction(reference, direct_user).await? {
			// A restriction narrows the Tenant policy; refuse exactly as an
			// unapproved version does, without disclosing the restriction.
			return Err(Error::Forbidden);
		}
	}
	// The distribution, catalog and policy locks taken by the first admission keep it
	// current for the rest of the scope's transaction; repeating it re-proves nothing.
	if let Some(entry) = scope.admitted(reference, action) {
		scope.remember(reference);
		return Ok(entry);
	}
	let document = scope.document(reference).await?.ok_or(Error::Forbidden)?;
	let entry: Entry = serde_json::from_value(document)?;
	if let Some(projection) = &entry.installation
		&& (projection.contract != 1 || projection.tenant != scope.tenant())
	{
		return Err(Error::Forbidden);
	}
	let resource = scope.resource(&entry);
	scope
		.require(&resource, action)
		.await
		.map_err(|error| match error {
			Error::Forbidden => tool.map_or(Error::Forbidden, |tool| policy_refusal(tool, action)),
			error => error,
		})?;
	scope.remember(reference);
	scope.admit(reference, action, &entry);
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
		let direct_user = scope.direct_user(&reference);
		if (!scope.inherited_lease()
			|| scope.approved(&reference)
			|| crate::registry::system::is_builtin(&entry))
			&& scope.permits_restriction(&reference, &direct_user).await?
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

/// One exact Catalog Approval mutation. Shared with Marketplace activation,
/// whose caller already holds its writer and admission lease.
pub struct CatalogChange<'a> {
	pub tenant: &'a str,
	pub node: &'a aidash_domain::nodes::NodeName,
	pub reference: &'a EntityRef,
	pub expected_revision: i64,
	pub enabled: bool,
	pub restriction: RestrictionChange,
	pub actor: &'a str,
}

/// Apply one mutation. All effects and the resulting history remain in the
/// caller's transaction.
pub async fn set_in_with_restriction(
	scope: &mut dyn crate::ports::catalog::CatalogMutation,
	change: CatalogChange<'_>,
) -> Result<aidash_domain::identity::catalog::Binding> {
	aidash_domain::identity::catalog::validate_revision(change.expected_revision)?;
	if let RestrictionChange::Replace(Some(restriction)) = &change.restriction {
		restriction.validate()?;
	}
	if !scope.registered(change.reference).await? {
		return Err(Error::NotFound("registry entry".into()));
	}
	let binding = scope
		.compare_and_set(
			change.tenant,
			change.node,
			change.reference,
			change.expected_revision,
			change.enabled,
			change.restriction,
		)
		.await?
		.ok_or_else(|| Error::Conflict("catalog revision changed".into()))?;
	scope.history(&binding, change.actor).await?;
	Ok(binding)
}

/// Create or change an approval without naming a restriction. The stored one
/// stays as it is, so re-enabling never silently widens an approval.
pub async fn set_in(
	scope: &mut dyn crate::ports::catalog::CatalogMutation,
	tenant: &str,
	node: &aidash_domain::nodes::NodeName,
	reference: &EntityRef,
	expected_revision: i64,
	enabled: bool,
	actor: &str,
) -> Result<aidash_domain::identity::catalog::Binding> {
	set_in_preserving(
		scope,
		tenant,
		node,
		reference,
		expected_revision,
		enabled,
		actor,
	)
	.await
}

pub async fn set_in_preserving(
	scope: &mut dyn crate::ports::catalog::CatalogMutation,
	tenant: &str,
	node: &aidash_domain::nodes::NodeName,
	reference: &EntityRef,
	expected_revision: i64,
	enabled: bool,
	actor: &str,
) -> Result<aidash_domain::identity::catalog::Binding> {
	set_in_with_restriction(
		scope,
		CatalogChange {
			tenant,
			node,
			reference,
			expected_revision,
			enabled,
			restriction: RestrictionChange::Preserve,
			actor,
		},
	)
	.await
}

/// Normalize a requested restriction so an empty one is stored as absent.
pub fn restriction_change(restriction: Option<CatalogRestriction>) -> RestrictionChange {
	match restriction {
		Some(restriction) => RestrictionChange::Replace(restriction.normalized()),
		None => RestrictionChange::Preserve,
	}
}

pub async fn set_catalog(
	scope: &mut dyn crate::ports::catalog::CatalogAdministrator,
	tenant: &str,
	node: &aidash_domain::nodes::NodeName,
	reference: &EntityRef,
	expected_revision: i64,
	enabled: bool,
	actor: &str,
) -> Result<aidash_domain::identity::catalog::Binding> {
	set_catalog_with_restriction(
		scope,
		CatalogChange {
			tenant,
			node,
			reference,
			expected_revision,
			enabled,
			restriction: RestrictionChange::Preserve,
			actor,
		},
	)
	.await
}

pub async fn set_catalog_with_restriction(
	scope: &mut dyn crate::ports::catalog::CatalogAdministrator,
	change: CatalogChange<'_>,
) -> Result<aidash_domain::identity::catalog::Binding> {
	super::require_deployment_administrator(scope.principal())?;
	aidash_domain::identity::catalog::validate_revision(change.expected_revision)?;
	scope.load_tenant(change.tenant).await?;
	let definition = scope.definition(change.reference).await?;
	if !aidash_domain::marketplace::installations::catalog_update_allowed(&definition) {
		return Err(Error::Forbidden);
	}
	set_in_with_restriction(scope, change).await
}

#[cfg(test)]
mod mutation_tests;

pub mod approvals;
pub mod retained;
pub mod review;
pub mod withdrawal;

pub fn attributes(entry: &Entry) -> serde_json::Value {
	serde_json::json!({"version":entry.version,"capabilities":entry.capabilities,"tags":entry.tags,"languages":entry.languages,"config":entry.config})
}
