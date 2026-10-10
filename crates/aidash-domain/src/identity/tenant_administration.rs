//! Tenant Administrator rules. Assignable Groups bound the only authority a
//! Tenant Administrator can confer; Operator authority is never Tenant-scoped.
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

/// Every action a Tenant's policy can permit to make a Tenant Administrator.
pub const ACTIONS: [&str; 6] = [
	REGISTRATION_READ,
	REGISTRATION_APPROVE,
	REGISTRATION_REJECT,
	MAPPING_READ,
	MAPPING_DISABLE,
	SUBJECT_GROUP_UPDATE,
];

/// Resource ID used when a capability or collection, not one item, is checked.
pub const COLLECTION: &str = "all";

/// The resource kind each action is evaluated against.
pub fn resource_kind(action: &str) -> &'static str {
	match action {
		REGISTRATION_READ | REGISTRATION_APPROVE | REGISTRATION_REJECT => "registration",
		MAPPING_READ | MAPPING_DISABLE => "mapping",
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
}

/// A Tenant Administrator never maps a person onto an existing subject it did
/// not create, because that subject may hold authority outside Assignable Groups.
pub fn approval_target(
	bundle: &PolicyBundle,
	subject: &str,
	previously_mapped: bool,
	groups: &BTreeSet<String>,
) -> Result<ApprovalTarget> {
	identifier(subject)?;
	if bundle.subjects.contains_key(subject) {
		if !previously_mapped {
			return Err(Error::Conflict("subject already exists".into()));
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

/// Adds a user subject holding only the requested Assignable Groups.
pub fn add_user_subject(
	bundle: &mut PolicyBundle,
	subject: &str,
	groups: &BTreeSet<String>,
) -> Result<()> {
	if approval_target(bundle, subject, false, groups)? != ApprovalTarget::NewSubject {
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

/// Replaces only a user subject's Assignable Group memberships. Returns whether
/// the bundle changed; every other group and role is retained.
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
	if entry.kind != SubjectKind::User {
		return Err(Error::Invalid(
			"only user subjects have Assignable Group memberships".into(),
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
