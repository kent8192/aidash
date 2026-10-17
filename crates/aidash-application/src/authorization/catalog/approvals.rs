//! A Tenant subject's Catalog Approvals. Each exact version needs
//! `catalog.approve`; an approval is refused while a dependency is unapproved,
//! and one request may approve a whole dependency closure. Approving an agent
//! version also makes the agent a subject of the Tenant's policy (ADR 0062).
use crate::{Error, Result, ports::catalog::CatalogApprovalScope};
use aidash_domain::{
	identity::{catalog::Binding, tenant_administration as rules},
	registry::{EntityRef, Entry},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
}

/// The exact versions a definition names directly, which must be approved (or
/// be built-ins that need no approval) before it is.
pub fn dependencies(entry: &Entry) -> Vec<EntityRef> {
	let config = &entry.config;
	let mut references = Vec::new();
	let mut push = |value: &Value| {
		if let (Some(id), Some(version)) = (
			value.get("id").and_then(Value::as_str),
			value.get("version").and_then(Value::as_str),
		) {
			references.push(EntityRef {
				id: id.into(),
				version: version.into(),
			});
		}
	};
	match entry.kind.as_str() {
		"agent" => {
			for field in ["model", "cluster"] {
				if let Some(value) = config.get(field) {
					push(value);
				}
			}
			for binding in config
				.get("bindings")
				.and_then(Value::as_array)
				.into_iter()
				.flatten()
			{
				if let Some(target) = binding.get("target") {
					push(target);
				}
			}
		}
		"bundle" => {
			for member in config
				.get("members")
				.and_then(Value::as_array)
				.into_iter()
				.flatten()
			{
				push(member);
			}
		}
		"cluster" => {
			if let Some(coordinator) = config.get("coordinator") {
				push(coordinator);
			}
		}
		_ => {}
	}
	references
}

/// Decide every approval in one transaction. The caller commits it.
pub async fn approve(
	scope: &mut dyn CatalogApprovalScope,
	approvals: &[Approval],
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
		// An installed copy is approved with its Marketplace activation.
		if !aidash_domain::marketplace::installations::catalog_update_allowed(&entry) {
			return Err(Error::Forbidden);
		}
		scope.require(&entry, CATALOG_APPROVE).await?;
		definitions.insert(key, entry);
	}
	let enabling: BTreeSet<_> = approvals
		.iter()
		.filter(|approval| approval.enabled)
		.map(|approval| (approval.entry.id.clone(), approval.entry.version.clone()))
		.collect();
	for approval in approvals.iter().filter(|approval| approval.enabled) {
		let entry = &definitions[&(approval.entry.id.clone(), approval.entry.version.clone())];
		for dependency in dependencies(entry) {
			let satisfied = crate::registry::system::builtin_reference(&dependency)
				|| enabling.contains(&(dependency.id.clone(), dependency.version.clone()))
				|| scope.approved(&dependency).await?;
			if !satisfied {
				return Err(Error::Invalid(format!(
					"CATALOG_DEPENDENCY_UNAPPROVED: {}@{}",
					dependency.id, dependency.version
				)));
			}
		}
	}
	let tenant = scope.tenant().to_owned();
	let actor = scope.actor().to_owned();
	let mut bindings = Vec::with_capacity(approvals.len());
	for approval in approvals {
		bindings.push(
			super::set_in(
				scope,
				&tenant,
				&approval.entry,
				approval.expected_revision,
				approval.enabled,
				&actor,
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
		if scope.policy().subjects.contains_key(&subject) {
			// Approving again leaves the subject as it is, enabled or disabled.
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
