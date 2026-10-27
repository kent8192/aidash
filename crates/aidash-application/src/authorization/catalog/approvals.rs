//! A Tenant subject's Catalog Approvals. Each exact version needs
//! `catalog.approve`; an approval is refused while a dependency is unapproved,
//! and one request may approve a whole dependency closure. Approving an agent
//! version also makes the agent a subject of the Tenant's policy (ADR 0062).
use crate::{Error, Result, ports::catalog::CatalogApprovalScope};
use aidash_domain::{
	identity::{
		catalog::{Binding, CatalogRestriction, RestrictionChange},
		tenant_administration as rules,
	},
	policy::SubjectKind,
	registry::{EntityRef, Entry, bindings::AgentBindings},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const CATALOG_APPROVE: &str = "catalog.approve";
const MAX_APPROVALS: usize = 128;

/// One decision on one exact version in the Tenant's Registry.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Approval {
	pub entry: EntityRef,
	pub enabled: bool,
	pub expected_revision: i64,
	/// Assignable Groups for the subject of an approved agent version; the
	/// default (`members`) applies when absent.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub groups: Option<BTreeSet<String>>,
	/// Who may use the version on this Node. Absent keeps an existing
	/// restriction (or leaves a new approval unrestricted); an empty
	/// restriction clears it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub restriction: Option<CatalogRestriction>,
}

/// The exact versions a definition names directly, which must be approved (or
/// be built-ins that need no approval) before it is: the edges Marketplace
/// distribution follows, plus an agent's context summarizer, which Run
/// admission also resolves.
pub fn dependencies(entry: &Entry, node: &str) -> Result<Vec<EntityRef>> {
	let mut dependencies: Vec<_> = crate::marketplace::definitions::refs(entry, node)?
		.into_iter()
		.map(|(reference, _)| reference)
		.collect();
	if entry.kind == "agent" {
		let config: AgentBindings = serde_json::from_value(entry.config.clone())
			.map_err(|error| Error::Invalid(format!("invalid Agent bindings: {error}")))?;
		dependencies.extend(
			config
				.context_summary()
				.map(|summary| summary.model.clone()),
		);
	}
	Ok(dependencies)
}

/// Decide every approval in one transaction. The caller commits it.
pub async fn approve(
	scope: &mut dyn CatalogApprovalScope,
	approvals: &[Approval],
) -> Result<Vec<Binding>> {
	approve_with_satisfied(scope, approvals, &BTreeSet::new(), &BTreeSet::new()).await
}

/// The same, treating `satisfied` as approved by an activation in the same
/// submission, so one review can approve a local definition whose dependency
/// is a Marketplace copy it activates in the same transaction.
pub async fn approve_with_satisfied(
	scope: &mut dyn CatalogApprovalScope,
	approvals: &[Approval],
	satisfied: &BTreeSet<(String, String)>,
	extra_subjects: &BTreeSet<String>,
) -> Result<Vec<Binding>> {
	if approvals.is_empty() || approvals.len() > MAX_APPROVALS {
		return Err(Error::Invalid("CATALOG_APPROVALS_BOUNDS".into()));
	}
	let mut definitions = BTreeMap::new();
	for approval in approvals {
		aidash_domain::identity::catalog::validate_revision(approval.expected_revision)?;
		let key = (approval.entry.id.clone(), approval.entry.version.clone());
		if definitions.contains_key(&key) {
			return Err(Error::Invalid("CATALOG_APPROVAL_DUPLICATE".into()));
		}
		let entry = scope.definition(&approval.entry).await?;
		if !aidash_domain::marketplace::installations::catalog_update_allowed(&entry) {
			return Err(Error::Forbidden);
		}
		scope.require(&entry, CATALOG_APPROVE).await?;
		definitions.insert(key, entry);
	}
	// A version approved by this request may be named by another item's
	// restriction, so an Approval Review can restrict a dependency to the
	// agent it is approving at the same time.
	let mut new_agents = BTreeSet::new();
	for approval in approvals.iter().filter(|approval| approval.enabled) {
		let entry = &definitions[&(approval.entry.id.clone(), approval.entry.version.clone())];
		if entry.kind == "agent" {
			new_agents.insert(aidash_domain::qualified_agent(
				scope.node(),
				&entry.id,
				&entry.version,
			));
		}
	}
	let existing_subjects = scope
		.policy()
		.subjects
		.keys()
		.cloned()
		.collect::<BTreeSet<_>>();
	let assignable_groups = scope
		.policy()
		.groups
		.iter()
		.filter(|(_, group)| group.assignable)
		.map(|(name, _)| name.clone())
		.collect::<BTreeSet<_>>();
	for approval in approvals.iter().filter(|approval| approval.enabled) {
		let Some(restriction) = &approval.restriction else {
			continue;
		};
		restriction.validate()?;
		if let Some(subject) = restriction.subjects.iter().find(|subject| {
			!existing_subjects.contains(*subject)
				&& !new_agents.contains(*subject)
				&& !extra_subjects.contains(*subject)
		}) {
			return Err(Error::Invalid(format!(
				"CATALOG_RESTRICTION_UNKNOWN_SUBJECT: {subject}"
			)));
		}
		if let Some(group) = restriction
			.groups
			.iter()
			.find(|group| !assignable_groups.contains(*group))
		{
			return Err(Error::Invalid(format!(
				"CATALOG_RESTRICTION_NOT_ASSIGNABLE: {group}"
			)));
		}
	}
	let requested: BTreeMap<_, _> = approvals
		.iter()
		.map(|approval| {
			(
				(approval.entry.id.clone(), approval.entry.version.clone()),
				approval.enabled,
			)
		})
		.collect();
	for approval in approvals.iter().filter(|approval| approval.enabled) {
		let entry = &definitions[&(approval.entry.id.clone(), approval.entry.version.clone())];
		for dependency in dependencies(entry, scope.node().deployment.as_str())? {
			let key = (dependency.id.clone(), dependency.version.clone());
			let available = crate::registry::system::builtin_reference(&dependency)
				|| satisfied.contains(&key)
				|| match requested.get(&key) {
					Some(enabled) => *enabled,
					None => scope.approved(&dependency).await?,
				};
			if !available {
				return Err(Error::Invalid(format!(
					"CATALOG_DEPENDENCY_UNAPPROVED: {}@{}",
					dependency.id, dependency.version
				)));
			}
		}
	}
	let tenant = scope.tenant().to_owned();
	let scope_node = scope.node().node.clone();
	let actor = scope.actor().to_owned();
	let mut bindings = Vec::with_capacity(approvals.len());
	for approval in approvals {
		bindings.push(
			super::set_in_with_restriction(
				scope,
				super::CatalogChange {
					tenant: &tenant,
					node: &scope_node,
					reference: &approval.entry,
					expected_revision: approval.expected_revision,
					enabled: approval.enabled,
					restriction: if approval.enabled {
						match &approval.restriction {
							Some(restriction) => {
								RestrictionChange::Replace(restriction.clone().normalized())
							}
							None => RestrictionChange::Preserve,
						}
					} else {
						// Withdrawal is never blocked by a restriction, and leaves
						// any stored one for the next approval.
						RestrictionChange::Preserve
					},
					actor: &actor,
				},
			)
			.await?,
		);
	}
	let mut changed = false;
	for approval in approvals.iter().filter(|approval| approval.enabled) {
		let entry = &definitions[&(approval.entry.id.clone(), approval.entry.version.clone())];
		if entry.kind != "agent" {
			continue;
		}
		let subject = aidash_domain::qualified_agent(scope.node(), &entry.id, &entry.version);
		if let Some(existing) = scope.policy().subjects.get(&subject) {
			if existing.kind != SubjectKind::Agent {
				return Err(Error::Conflict("AGENT_SUBJECT_KIND".into()));
			}
			if approval.groups.is_some() {
				return Err(Error::Conflict("AGENT_SUBJECT_EXISTS".into()));
			}
			continue;
		}
		let default = rules::default_agent_groups(scope.policy());
		let groups = approval.groups.clone().unwrap_or_else(|| default.clone());
		if groups != default {
			scope
				.require_subject(&subject, rules::SUBJECT_GROUP_UPDATE, &groups)
				.await?;
		}
		rules::add_agent_subject(scope.policy(), &subject, &groups)?;
		changed = true;
	}
	if changed {
		scope.save_policy().await?;
	}
	Ok(bindings)
}

#[cfg(test)]
mod tests;
