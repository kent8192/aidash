//! Portable catalog approval state, separate from its persisted representation.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Who may use an approved definition on its Node (ADR 0029, ADR 0095). A
/// restriction names subjects directly and/or Assignable Groups; their
/// members may use it. It only narrows what the Tenant's policy already
/// allows, so it can never grant authority.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogRestriction {
	#[serde(default)]
	pub subjects: BTreeSet<String>,
	#[serde(default)]
	pub groups: BTreeSet<String>,
}

impl CatalogRestriction {
	/// A restriction with no member names nobody; it is the same as no
	/// restriction at all.
	pub fn is_unrestricted(&self) -> bool {
		self.subjects.is_empty() && self.groups.is_empty()
	}

	/// Reject names that cannot identify a Tenant subject or group.
	pub fn validate(&self) -> crate::Result<()> {
		if self.subjects.len() > 512 || self.groups.len() > 512 {
			return Err(crate::Error::Invalid(
				"a Catalog Approval restriction names at most 512 subjects and groups".into(),
			));
		}
		for subject in &self.subjects {
			crate::policy::identifier(subject)?;
		}
		for group in &self.groups {
			crate::policy::identifier(group)?;
		}
		Ok(())
	}

	/// Whether `subject` may use the definition, given its Assignable Groups.
	/// An unrestricted approval admits everyone the Tenant's policy allows.
	pub fn allows(&self, subject: &str, groups: &BTreeSet<String>) -> bool {
		self.is_unrestricted()
			|| self.subjects.contains(subject)
			|| !self.groups.is_disjoint(groups)
	}

	/// Store an empty restriction as no restriction, so every existing reader
	/// keeps treating it as unrestricted.
	pub fn normalized(self) -> Option<Self> {
		(!self.is_unrestricted()).then_some(self)
	}

	/// Everyone either restriction admits. Widening an approval keeps the
	/// existing members and adds the new ones.
	pub fn union(&self, other: &Self) -> Self {
		Self {
			subjects: self.subjects.union(&other.subjects).cloned().collect(),
			groups: self.groups.union(&other.groups).cloned().collect(),
		}
	}

	/// Whether this restriction names everything `other` does.
	pub fn contains(&self, other: &Self) -> bool {
		other.subjects.is_subset(&self.subjects) && other.groups.is_subset(&self.groups)
	}
}

/// A requested change to a Catalog Approval's restriction. An approval that
/// does not name one keeps the stored restriction, so re-enabling an approval
/// never silently widens it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RestrictionChange {
	/// Leave the stored restriction as it is; a new approval stays
	/// unrestricted.
	#[default]
	Preserve,
	/// Replace the restriction. `None` and an empty restriction both clear it.
	Replace(Option<CatalogRestriction>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "Binding")]
pub struct Binding {
	pub tenant: String,
	/// The Node the approval applies to.
	pub node: crate::nodes::NodeName,
	pub entry_id: String,
	pub entry_version: String,
	pub enabled: bool,
	pub revision: i64,
	/// Who may use the definition on this Node; absent means everyone the
	/// Tenant's policy allows.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub restriction: Option<CatalogRestriction>,
}

pub fn validate_revision(expected_revision: i64) -> crate::Result<()> {
	if !(0..i64::MAX).contains(&expected_revision) {
		return Err(crate::Error::Invalid("invalid catalog revision".into()));
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn groups(values: &[&str]) -> BTreeSet<String> {
		values.iter().map(|value| (*value).to_owned()).collect()
	}

	#[rstest::rstest]
	#[case::initial(0, true)]
	#[case::last_mutable(i64::MAX - 1, true)]
	#[case::negative(-1, false)]
	#[case::overflow(i64::MAX, false)]
	fn revisions_allow_the_initial_and_last_nonoverflowing_updates(
		#[case] expected: i64,
		#[case] allowed: bool,
	) {
		assert_eq!(super::validate_revision(expected).is_ok(), allowed);
	}

	#[rstest::rstest]
	fn an_approval_without_a_restriction_admits_everyone() {
		let restriction = CatalogRestriction::default();
		assert!(restriction.is_unrestricted());
		assert!(restriction.allows("alice", &groups(&["members"])));
		assert!(restriction.allows("agent@1", &BTreeSet::new()));
		assert_eq!(restriction.clone().normalized(), None);
	}

	#[rstest::rstest]
	fn a_named_subject_admits_only_that_subject() {
		let restriction = CatalogRestriction {
			subjects: groups(&["alice"]),
			groups: BTreeSet::new(),
		};
		assert!(restriction.allows("alice", &BTreeSet::new()));
		assert!(!restriction.allows("bob", &groups(&["members"])));
	}

	#[rstest::rstest]
	fn an_assignable_group_admits_its_current_members() {
		let restriction = CatalogRestriction {
			subjects: BTreeSet::new(),
			groups: groups(&["reviewers"]),
		};
		assert!(restriction.allows("bob", &groups(&["reviewers", "members"])));
		assert!(!restriction.allows("carol", &groups(&["members"])));
		assert!(!restriction.allows("carol", &BTreeSet::new()));
	}

	#[rstest::rstest]
	fn naming_a_subject_and_a_group_admits_either() {
		let restriction = CatalogRestriction {
			subjects: groups(&["alice"]),
			groups: groups(&["reviewers"]),
		};
		assert!(restriction.allows("alice", &BTreeSet::new()));
		assert!(restriction.allows("bob", &groups(&["reviewers"])));
		assert!(!restriction.allows("carol", &groups(&["members"])));
	}

	#[rstest::rstest]
	fn an_empty_restriction_normalizes_to_unrestricted() {
		let restriction = CatalogRestriction {
			subjects: BTreeSet::new(),
			groups: BTreeSet::new(),
		};
		assert_eq!(restriction.clone().normalized(), None);
	}

	#[rstest::rstest]
	fn restriction_names_must_be_identifiers() {
		for restriction in [
			CatalogRestriction {
				subjects: groups(&["bad name"]),
				groups: BTreeSet::new(),
			},
			CatalogRestriction {
				subjects: BTreeSet::new(),
				groups: groups(&["*"]),
			},
		] {
			assert!(restriction.validate().is_err());
		}
		let valid = CatalogRestriction {
			subjects: groups(&["alice"]),
			groups: groups(&["reviewers"]),
		};
		assert!(valid.validate().is_ok());
	}

	#[rstest::rstest]
	fn a_union_keeps_existing_members_and_adds_only_the_new_ones() {
		let current = CatalogRestriction {
			subjects: groups(&["alice"]),
			groups: groups(&["reviewers"]),
		};
		let proposed = CatalogRestriction {
			subjects: groups(&["alice", "writer@1"]),
			groups: BTreeSet::new(),
		};
		let union = current.union(&proposed);
		assert_eq!(union.subjects, groups(&["alice", "writer@1"]));
		assert_eq!(union.groups, groups(&["reviewers"]));
		assert!(union.contains(&current));
		assert!(union.contains(&proposed));
	}

	#[rstest::rstest]
	fn a_binding_without_a_restriction_keeps_its_previous_serialized_shape() {
		let binding = Binding {
			tenant: "acme".into(),
			node: "brave-otter-k3m9q2xa".parse().unwrap(),
			entry_id: "skill".into(),
			entry_version: "1.0.0".into(),
			enabled: true,
			revision: 1,
			restriction: None,
		};
		assert_eq!(
			serde_json::to_value(&binding).unwrap(),
			serde_json::json!({"tenant":"acme","node":"brave-otter-k3m9q2xa","entry_id":"skill","entry_version":"1.0.0","enabled":true,"revision":1})
		);
	}
}
