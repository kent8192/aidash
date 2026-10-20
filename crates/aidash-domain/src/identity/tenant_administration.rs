//! Tenant Administrator rules. Without permission to replace the policy,
//! Assignable Groups bound the only authority a Tenant Administrator can
//! confer; Deployment Administrator authority is never Tenant-scoped.
use crate::policy::{PolicyBundle, Subject, SubjectKind, identifier};
use crate::{Error, Result};
use serde_json::json;
use std::collections::BTreeSet;

pub const REGISTRATION_READ: &str = "registration.read";
pub const REGISTRATION_APPROVE: &str = "registration.approve";
pub const REGISTRATION_REJECT: &str = "registration.reject";
pub const MAPPING_READ: &str = "mapping.read";
pub const MAPPING_DISABLE: &str = "mapping.disable";
pub const SUBJECT_GROUP_UPDATE: &str = "subject.group.update";
pub const INVITATION_CREATE: &str = "invitation.create";
pub const INVITATION_READ: &str = "invitation.read";
pub const INVITATION_REVOKE: &str = "invitation.revoke";
/// Reading the Tenant's policy, its revisions and simulated decisions.
pub const POLICY_READ: &str = "policy.read";
/// Replacing the Tenant's whole policy, which is every authority within it.
pub const POLICY_UPDATE: &str = "policy.update";
/// Reading the Tenant's decision log.
pub const DECISION_READ: &str = "decision.read";
/// Issuing a Subject Credential for one's own subject (ADR 0063).
pub const CREDENTIAL_CREATE: &str = "credential.create";
/// Issuing a Subject Credential for a service or agent subject that no
/// Mapping ever named (ADR 0063).
pub const CREDENTIAL_ISSUE: &str = "credential.issue";
pub const CREDENTIAL_READ: &str = "credential.read";
pub const CREDENTIAL_REVOKE: &str = "credential.revoke";
pub const PEER_MAPPING_READ: &str = "peer_mapping.read";
pub const PEER_MAPPING_UPDATE: &str = "peer_mapping.update";
/// Setting the Tenant's Workbench Test Limits within the Deployment's ceilings.
pub const TEST_LIMITS_UPDATE: &str = "workbench.test_limits.update";
/// Writing the Tenant's Workbench Test Profiles.
pub const TEST_PROFILE_UPDATE: &str = "workbench.test_profile.update";
/// Reading a member's default Approval Mode and Approval Rules (ADR 0076). The
/// owner alone writes them, so there is no matching update action.
pub const APPROVAL_READ: &str = "approval.read";

/// Every action a Tenant's policy can permit to make a Tenant Administrator.
pub const ACTIONS: [&str; 20] = [
	REGISTRATION_READ,
	REGISTRATION_APPROVE,
	REGISTRATION_REJECT,
	MAPPING_READ,
	MAPPING_DISABLE,
	SUBJECT_GROUP_UPDATE,
	INVITATION_CREATE,
	INVITATION_READ,
	INVITATION_REVOKE,
	POLICY_READ,
	POLICY_UPDATE,
	DECISION_READ,
	CREDENTIAL_ISSUE,
	CREDENTIAL_READ,
	CREDENTIAL_REVOKE,
	PEER_MAPPING_READ,
	PEER_MAPPING_UPDATE,
	TEST_LIMITS_UPDATE,
	TEST_PROFILE_UPDATE,
	APPROVAL_READ,
];

/// The actions an administrator needs to keep a Tenant running: deciding
/// requests, managing Mappings and groups, inviting people, repairing the
/// policy itself (ADR 0026), and deleting the Tenant, so nobody leaves behind a
/// Tenant no one can delete (ADR 0060). The last-administrator check requires
/// one other subject holding all of them; what a policy writer can restore is
/// not here.
pub const ADMINISTRATION_ACTIONS: [&str; 10] = [
	REGISTRATION_READ,
	REGISTRATION_APPROVE,
	REGISTRATION_REJECT,
	MAPPING_READ,
	MAPPING_DISABLE,
	SUBJECT_GROUP_UPDATE,
	INVITATION_CREATE,
	POLICY_READ,
	POLICY_UPDATE,
	crate::identity::tenancy::TENANT_DELETE,
];

/// Creating a definition or version in the Tenant's Registry.
pub const REGISTRY_CREATE: &str = "registry.create";
/// Deciding a Catalog Approval.
pub const CATALOG_APPROVE: &str = "catalog.approve";
/// Installing a Marketplace package as the Tenant's own copy.
pub const MARKETPLACE_INSTALL: &str = "marketplace.install";
/// The Registry actions the dashboard shows controls for (ADR 0029).
pub const REGISTRY_ACTIONS: [&str; 3] = [REGISTRY_CREATE, CATALOG_APPROVE, MARKETPLACE_INSTALL];

/// Resource ID used when a capability or collection, not one item, is checked.
pub const COLLECTION: &str = "all";

/// The resource kind each action is evaluated against.
pub fn resource_kind(action: &str) -> &'static str {
	match action {
		REGISTRATION_READ | REGISTRATION_APPROVE | REGISTRATION_REJECT => "registration",
		MAPPING_READ | MAPPING_DISABLE => "mapping",
		INVITATION_CREATE | INVITATION_READ | INVITATION_REVOKE => "invitation",
		POLICY_READ | POLICY_UPDATE => "policy",
		DECISION_READ => "decision",
		CREDENTIAL_READ | CREDENTIAL_REVOKE => "credential",
		PEER_MAPPING_READ | PEER_MAPPING_UPDATE => "peer_mapping",
		TEST_LIMITS_UPDATE => "test_limits",
		TEST_PROFILE_UPDATE => "test_profile",
		APPROVAL_READ => "subject",
		crate::approval_rules::MODE_AUTO | crate::approval_rules::MODE_BYPASS => {
			crate::approval_rules::MODE_RESOURCE
		}
		crate::identity::tenancy::TENANT_UPDATE | crate::identity::tenancy::TENANT_DELETE => {
			"tenant"
		}
		REGISTRY_CREATE | CATALOG_APPROVE => "registry",
		MARKETPLACE_INSTALL => "package",
		_ => "subject",
	}
}

pub fn assignable_groups(bundle: &PolicyBundle) -> BTreeSet<String> {
	bundle
		.groups
		.iter()
		.filter(|(_, group)| group.assignable)
		.map(|(name, _)| name.clone())
		.collect()
}

fn require_assignable(bundle: &PolicyBundle, groups: &BTreeSet<String>) -> Result<()> {
	for name in groups {
		if !bundle
			.groups
			.get(name)
			.is_some_and(|group| group.assignable)
		{
			return Err(Error::Invalid(format!("not an Assignable Group: {name}")));
		}
	}
	Ok(())
}

/// What an approval by a Tenant Administrator maps the External Identity to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalTarget {
	/// A user subject created by this approval with only Assignable Groups.
	NewSubject,
	/// The unchanged subject this External Identity was previously mapped to.
	Reapproval,
	/// The subject a member who left returns to through an Invitation. Its
	/// Assignable Groups become the approved ones, so leaving and returning
	/// never revives authority the Invitation did not offer.
	Rejoin,
}

/// A Tenant Administrator never maps a person onto an existing subject it did
/// not create, because that subject may hold authority outside Assignable Groups.
/// `invited` is whether the request came through an Invitation.
pub fn approval_target(
	bundle: &PolicyBundle,
	subject: &str,
	previously_mapped: bool,
	groups: &BTreeSet<String>,
	invited: bool,
) -> Result<ApprovalTarget> {
	identifier(subject)?;
	if bundle.subjects.contains_key(subject) {
		if !previously_mapped {
			return Err(Error::Conflict("subject already exists".into()));
		}
		if invited {
			require_assignable(bundle, groups)?;
			return Ok(ApprovalTarget::Rejoin);
		}
		if !groups.is_empty() {
			return Err(Error::Invalid(
				"re-approval keeps the existing subject's groups".into(),
			));
		}
		return Ok(ApprovalTarget::Reapproval);
	}
	require_assignable(bundle, groups)?;
	Ok(ApprovalTarget::NewSubject)
}

/// Gives a rejoining subject exactly `groups` among the Assignable Groups,
/// keeping the groups only a Deployment Administrator assigns.
pub fn rejoin(bundle: &mut PolicyBundle, subject: &str, groups: &BTreeSet<String>) -> Result<()> {
	require_assignable(bundle, groups)?;
	let assignable = assignable_groups(bundle);
	let entry = bundle
		.subjects
		.get_mut(subject)
		.ok_or_else(|| Error::Conflict("subject does not exist".into()))?;
	entry.groups.retain(|group| !assignable.contains(group));
	entry.groups.extend(groups.iter().cloned());
	bundle.validate()
}

/// Adds a user subject holding only the requested Assignable Groups.
pub fn add_user_subject(
	bundle: &mut PolicyBundle,
	subject: &str,
	groups: &BTreeSet<String>,
) -> Result<()> {
	if approval_target(bundle, subject, false, groups, false)? != ApprovalTarget::NewSubject {
		return Err(Error::Conflict("subject already exists".into()));
	}
	bundle.subjects.insert(
		subject.into(),
		Subject {
			kind: SubjectKind::User,
			groups: groups.clone(),
			roles: BTreeSet::new(),
			attributes: json!({}),
			enabled: true,
			delegated_by: None,
		},
	);
	bundle.validate()
}

/// The groups an agent subject joins when its Catalog Approval names none:
/// `members` when the policy marks it Assignable, otherwise none (ADR 0062).
pub fn default_agent_groups(bundle: &PolicyBundle) -> BTreeSet<String> {
	assignable_groups(bundle)
		.into_iter()
		.filter(|group| group == crate::identity::tenancy::MEMBERS)
		.collect()
}

/// Adds the agent subject of an approved agent version. It has no delegator,
/// so its authority is its groups intersected with each Run's chain.
pub fn add_agent_subject(
	bundle: &mut PolicyBundle,
	subject: &str,
	groups: &BTreeSet<String>,
) -> Result<()> {
	if bundle.subjects.contains_key(subject) {
		return Err(Error::Conflict("subject already exists".into()));
	}
	require_assignable(bundle, groups)?;
	bundle.subjects.insert(
		subject.into(),
		Subject {
			kind: SubjectKind::Agent,
			groups: groups.clone(),
			roles: BTreeSet::new(),
			attributes: json!({}),
			enabled: true,
			delegated_by: None,
		},
	);
	bundle.validate()
}

/// Replaces only a user or agent subject's Assignable Group memberships.
/// Returns whether the bundle changed; every other group and role is retained.
pub fn replace_assignable_memberships(
	bundle: &mut PolicyBundle,
	subject: &str,
	groups: &BTreeSet<String>,
) -> Result<bool> {
	require_assignable(bundle, groups)?;
	let assignable = assignable_groups(bundle);
	let entry = bundle
		.subjects
		.get_mut(subject)
		.ok_or_else(|| Error::Invalid(format!("unknown subject: {subject}")))?;
	if !matches!(entry.kind, SubjectKind::User | SubjectKind::Agent) {
		return Err(Error::Invalid(
			"only user and agent subjects have Assignable Group memberships".into(),
		));
	}
	let next: BTreeSet<String> = entry
		.groups
		.difference(&assignable)
		.chain(groups)
		.cloned()
		.collect();
	if next == entry.groups {
		return Ok(false);
	}
	entry.groups = next;
	bundle.validate()?;
	Ok(true)
}

/// A subject's current Assignable Group memberships.
pub fn memberships(bundle: &PolicyBundle, subject: &str) -> BTreeSet<String> {
	let assignable = assignable_groups(bundle);
	bundle
		.subjects
		.get(subject)
		.map(|entry| entry.groups.intersection(&assignable).cloned().collect())
		.unwrap_or_default()
}

#[cfg(test)]
mod tests;
