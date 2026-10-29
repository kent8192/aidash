//! One Approval Review: the transitive closure of a root on one Node, the
//! readiness of every exact version and the one submission that decides what
//! the caller reviewed.
use super::approvals::{self, Approval};
use crate::{
	Error, Result,
	marketplace::operations::approval_set::{ApprovalSelection, PendingSelection},
	ports::catalog_review::ApprovalReviewScope,
	registry::system,
};
use aidash_domain::{
	identity::catalog::{CatalogRestriction, validate_revision},
	nodes::NodeName,
	registry::{EntityRef, Entry, Localized, rules::digest},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MAX_CLOSURE: usize = 512;

/// Whether one exact version can be used now, and if not, what stands in the
/// way. `Approved` means the Node's approval is enabled; a separate `widen`
/// reports an approved restriction that no longer admits every consumer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
	Approved,
	/// This principal holds `catalog.approve` and can decide it.
	Approvable,
	/// Another subject holds `catalog.approve`.
	NeedsOtherApprover,
	/// An installed copy whose installation must be activated on this Node.
	NeedsMarketplaceActivation,
	/// The dependency is not registered in the Tenant.
	Missing,
	/// The Deployment does not offer the provider the definition needs.
	ProviderUnavailable,
	/// A Tenant Secret or connection the definition names is absent.
	SecretOrConnectionMissing,
}

/// The installed copy a review item can activate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReviewInstallation {
	pub installation: String,
	pub revision: i64,
	/// The immutable revision digest the activation fences on.
	pub digest: String,
	pub expected_activation_revision: i64,
	pub active_revision: Option<i64>,
}

/// One exact definition in the reviewed closure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReviewItem {
	/// Readable agent subjects that directly use this item.
	pub consumers: Vec<String>,
	pub reference: EntityRef,
	pub kind: String,
	pub name: Localized,
	pub description: Localized,
	/// `digest(entry)`; absent only when the dependency is missing.
	pub digest: Option<String>,
	pub status: ReviewStatus,
	pub approved: bool,
	/// Whether this principal can decide it with the visible review information.
	pub can_approve: bool,
	/// The current Catalog Approval revision, `0` when none exists.
	pub expected_revision: i64,
	/// The restriction already stored on this Node.
	pub restriction: Option<CatalogRestriction>,
	/// The restriction this Review proposes for the direct consumers.
	pub proposed_restriction: Option<CatalogRestriction>,
	/// An existing restriction would gain consumers.
	pub widen: bool,
	pub installation: Option<ReviewInstallation>,
}

impl ReviewItem {
	/// This principal can still move the item forward: decide or activate it,
	/// or widen an approved restriction that no longer admits every consumer.
	pub fn awaits_approver(&self) -> bool {
		self.can_approve && (self.status != ReviewStatus::Approved || self.widen)
	}
}

/// The whole review of one root on one Node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ApprovalReview {
	pub node: NodeName,
	pub root: EntityRef,
	pub items: Vec<ReviewItem>,
	/// Closure definitions this principal may not read, counted but never named.
	pub hidden: usize,
	/// Stable hash over every reviewed exact version and its digest.
	pub closure_digest: String,
	/// Every named item is approved, admits its consumers and needs no widening.
	pub ready: bool,
}

/// One local approval the caller reviewed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewApproval {
	pub reference: EntityRef,
	pub digest: String,
	pub expected_revision: i64,
	/// An omitted restriction keeps the stored one; empty clears it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub restriction: Option<CatalogRestriction>,
}

/// One Marketplace activation the caller reviewed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewActivation {
	pub installation: String,
	pub revision: i64,
	/// The immutable revision digest (`Revision.digest`, no `sha256:` prefix).
	pub digest: String,
	pub expected_activation_revision: i64,
	/// An omitted restriction keeps the stored one; empty clears it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub restriction: Option<CatalogRestriction>,
}

/// The one submission that decides a reviewed subset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalReviewSubmission {
	pub node: NodeName,
	pub root: EntityRef,
	pub closure_digest: String,
	/// Groups for a newly approved root agent subject.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub root_groups: Option<BTreeSet<String>>,
	/// An open Catalog Approval Request this submission answers.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request: Option<uuid::Uuid>,
	#[serde(default)]
	pub items: Vec<ReviewApproval>,
	#[serde(default)]
	pub installations: Vec<ReviewActivation>,
}

/// The validated write plan. The server applies it in one transaction.
pub struct ReviewPlan {
	pub approvals: Vec<Approval>,
	pub installations: Vec<ReviewActivation>,
	/// Definitions the same submission approves by activation.
	pub satisfied_by_activation: BTreeSet<(String, String)>,
	pub activated: BTreeSet<(String, String)>,
}

/// Compute the reviewed closure under the caller's authority.
pub async fn review(
	scope: &mut dyn ApprovalReviewScope,
	root: &EntityRef,
) -> Result<ApprovalReview> {
	let root_entry = scope
		.definition(root)
		.await?
		.ok_or_else(|| Error::NotFound("registry entry".into()))?;
	scope.require_read(&root_entry).await?;
	let node_address = scope.node().clone();
	let node = node_address.node.clone();

	let mut queue = vec![root_entry.clone()];
	let mut seen = BTreeSet::new();
	let mut entries: BTreeMap<(String, String), Entry> = BTreeMap::new();
	let mut missing: BTreeMap<(String, String), bool> = BTreeMap::new();
	let mut readability: BTreeMap<(String, String), bool> = BTreeMap::new();
	let mut edges = BTreeMap::new();
	let mut hidden = 0usize;
	while let Some(entry) = queue.pop() {
		let key = (entry.id.clone(), entry.version.clone());
		if !seen.insert(key.clone()) {
			continue;
		}
		if seen.len() > MAX_CLOSURE {
			return Err(Error::Invalid(
				"approval review closure exceeds 512 definitions".into(),
			));
		}
		let dependencies = approvals::dependencies(&entry, node_address.deployment.as_str())?;
		edges.insert(
			key.clone(),
			dependencies
				.iter()
				.map(|d| (d.id.clone(), d.version.clone()))
				.collect::<Vec<_>>(),
		);
		let needs_approval = !system::is_builtin(&entry);
		let readable = scope.can_read(&entry).await?;
		if !readable && needs_approval && key != (root.id.clone(), root.version.clone()) {
			hidden += 1;
		}
		readability.insert(key.clone(), readable);
		entries.insert(key.clone(), entry.clone());
		for dependency in dependencies {
			let key = (dependency.id.clone(), dependency.version.clone());
			if seen.contains(&key) {
				continue;
			}
			if let Some(visible) = missing.get_mut(&key) {
				*visible |= readable;
				continue;
			}
			match scope.definition(&dependency).await? {
				Some(entry) => queue.push(entry),
				None => {
					missing.insert(key, readable);
				}
			}
		}
	}

	// The digest covers every closure definition that needs approval, including
	// hidden ones, so a client cannot hide a change behind a shorter set.
	let mut digest_parts = vec![];
	for (key, entry) in &entries {
		if !system::is_builtin(entry) {
			digest_parts.push(format!(
				"{}@{}:{}",
				key.0,
				key.1,
				digest(&serde_json::to_value(entry)?)
			));
		}
	}
	for key in missing.keys() {
		digest_parts.push(format!("{}@{}:missing", key.0, key.1));
	}
	digest_parts.sort();
	let closure_digest = digest(&serde_json::json!(digest_parts));

	// Propagate nearest agents along every path, including paths discovered after
	// a definition was expanded and cycles back to the root.
	let mut consumers: BTreeMap<(String, String), BTreeSet<(String, String)>> = BTreeMap::new();
	loop {
		let mut changed = false;
		for (key, targets) in &edges {
			let incoming = if entries[key].kind == "agent" {
				BTreeSet::from([key.clone()])
			} else {
				consumers.get(key).cloned().unwrap_or_default()
			};
			for target in targets {
				let set = consumers.entry(target.clone()).or_default();
				let before = set.len();
				set.extend(incoming.iter().cloned());
				changed |= set.len() != before;
			}
		}
		if !changed {
			break;
		}
	}
	let mut groups = BTreeMap::new();
	for (key, entry) in &entries {
		if entry.kind == "agent" {
			let subject = aidash_domain::qualified_agent(&node_address, &key.0, &key.1);
			groups.insert(
				key.clone(),
				(subject.clone(), scope.subject_groups(&subject).await?),
			);
		}
	}
	let hidden_subjects: BTreeSet<_> = groups
		.iter()
		.filter(|(key, _)| !readability.get(*key).copied().unwrap_or(false))
		.map(|(_, (subject, _))| subject.clone())
		.collect();
	let mut items = vec![];
	for (key, entry) in &entries {
		if system::is_builtin(entry) {
			continue;
		}
		if !readability.get(key).copied().unwrap_or(false)
			&& *key != (root.id.clone(), root.version.clone())
		{
			continue;
		}
		let agents = consumers.get(key).cloned().unwrap_or_default();
		let hidden_consumer = agents
			.iter()
			.any(|agent| !readability.get(agent).copied().unwrap_or(false));
		let visible = agents
			.iter()
			.filter(|agent| readability.get(*agent).copied().unwrap_or(false))
			.filter_map(|agent| groups.get(agent).cloned())
			.collect::<Vec<_>>();
		items.push(
			review_item(
				scope,
				entry,
				&visible,
				&hidden_subjects,
				hidden_consumer,
				*key == (root.id.clone(), root.version.clone()),
			)
			.await?,
		);
	}
	for (key, visible) in &missing {
		if !visible {
			hidden += 1;
			continue;
		}
		let hidden_consumer = consumers
			.get(key)
			.into_iter()
			.flatten()
			.any(|agent| !readability.get(agent).copied().unwrap_or(false));
		items.push(ReviewItem {
			consumers: consumers
				.get(key)
				.into_iter()
				.flatten()
				.filter(|agent| readability.get(*agent).copied().unwrap_or(false))
				.filter_map(|agent| groups.get(agent).map(|(subject, _)| subject.clone()))
				.collect(),
			reference: EntityRef {
				id: key.0.clone(),
				version: key.1.clone(),
			},
			kind: String::new(),
			name: Localized::new(),
			description: Localized::new(),
			digest: None,
			status: if hidden_consumer {
				ReviewStatus::NeedsOtherApprover
			} else {
				ReviewStatus::Missing
			},
			approved: false,
			can_approve: false,
			expected_revision: 0,
			restriction: None,
			proposed_restriction: None,
			widen: false,
			installation: None,
		});
	}
	items.sort_by(|left, right| {
		let left_root = left.reference == *root;
		let right_root = right.reference == *root;
		right_root
			.cmp(&left_root)
			.then_with(|| left.reference.id.cmp(&right.reference.id))
			.then_with(|| left.reference.version.cmp(&right.reference.version))
	});
	for item in &mut items {
		item.consumers.sort();
	}
	let ready = hidden == 0
		&& items
			.iter()
			.all(|item| item.status == ReviewStatus::Approved && !item.widen);
	Ok(ApprovalReview {
		node,
		root: root.clone(),
		items,
		hidden,
		closure_digest,
		ready,
	})
}

async fn review_item(
	scope: &mut dyn ApprovalReviewScope,
	entry: &Entry,
	consumers: &[(String, BTreeSet<String>)],
	hidden_subjects: &BTreeSet<String>,
	hidden_consumer: bool,
	is_root: bool,
) -> Result<ReviewItem> {
	let approval = scope
		.approval(&EntityRef {
			id: entry.id.clone(),
			version: entry.version.clone(),
		})
		.await?;
	let installed = match &entry.installation {
		Some(projection) => scope.installed(projection).await?,
		None => None,
	};
	let installed_missing = entry.installation.is_some() && installed.is_none();
	let approved = approval.as_ref().is_some_and(|approval| approval.enabled);
	let current = approval
		.as_ref()
		.and_then(|approval| approval.restriction.clone());
	let hidden_consumer = hidden_consumer
		|| current
			.as_ref()
			.is_some_and(|restriction| !restriction.subjects.is_disjoint(hidden_subjects));
	let expected_revision = approval.as_ref().map_or(0, |approval| approval.revision);
	let provider_available = scope.provider_available(entry).await?;
	let source_available = scope.source_slot_available(entry).await?;
	let can_approve = scope.can_approve(entry).await? && !hidden_consumer;
	let status = if hidden_consumer {
		ReviewStatus::NeedsOtherApprover
	} else if installed_missing {
		ReviewStatus::Missing
	} else if provider_available {
		if source_available {
			match (&installed, approved) {
				(Some(installed), _) if !approved || !installed.active => {
					ReviewStatus::NeedsMarketplaceActivation
				}
				(_, true) => ReviewStatus::Approved,
				(_, false) if can_approve => ReviewStatus::Approvable,
				(_, false) => ReviewStatus::NeedsOtherApprover,
			}
		} else {
			ReviewStatus::SecretOrConnectionMissing
		}
	} else {
		ReviewStatus::ProviderUnavailable
	};
	let missing: BTreeSet<String> = consumers
		.iter()
		.filter(|(subject, groups)| {
			current
				.as_ref()
				.is_none_or(|restriction| !restriction.allows(subject, groups))
		})
		.map(|(subject, _)| subject.clone())
		.collect();
	let (proposed_restriction, widen) = if hidden_consumer {
		(None, false)
	} else if consumers.is_empty() || (is_root && current.is_none()) {
		(current.clone(), false)
	} else if let Some(current) = &current {
		if missing.is_empty() {
			(Some(current.clone()), false)
		} else {
			(
				Some(current.union(&CatalogRestriction {
					subjects: missing,
					groups: BTreeSet::new(),
				})),
				true,
			)
		}
	} else {
		(
			Some(CatalogRestriction {
				subjects: missing,
				groups: BTreeSet::new(),
			}),
			false,
		)
	};
	Ok(ReviewItem {
		consumers: consumers
			.iter()
			.map(|(subject, _)| subject.clone())
			.collect(),
		reference: EntityRef {
			id: entry.id.clone(),
			version: entry.version.clone(),
		},
		kind: entry.kind.clone(),
		name: entry.name.clone(),
		description: entry.description.clone(),
		digest: Some(digest(&serde_json::to_value(entry)?)),
		status,
		approved,
		can_approve,
		expected_revision,
		restriction: if hidden_consumer { None } else { current },
		proposed_restriction,
		widen,
		installation: installed.map(|installed| ReviewInstallation {
			installation: installed.installation.id,
			revision: installed.revision,
			digest: installed.digest,
			expected_activation_revision: installed.activation_revision,
			active_revision: installed.installation.active_revision,
		}),
	})
}

/// Validate a submission against the freshly recomputed review and return the
/// exact write plan. Any stale fence or unapproved authority rejects the whole
/// submission before a write.
pub fn plan(review: &ApprovalReview, submission: &ApprovalReviewSubmission) -> Result<ReviewPlan> {
	if submission.root != review.root || submission.node != review.node {
		return Err(Error::Conflict(
			"APPROVAL_REVIEW_STALE: root or node changed".into(),
		));
	}
	if submission.closure_digest != review.closure_digest {
		return Err(Error::Conflict(
			"APPROVAL_REVIEW_STALE: closure changed".into(),
		));
	}
	if submission.items.is_empty() && submission.installations.is_empty() {
		return Err(Error::Invalid("APPROVAL_REVIEW_EMPTY".into()));
	}
	let by_reference: BTreeMap<(&str, &str), &ReviewItem> = review
		.items
		.iter()
		.map(|item| {
			(
				(item.reference.id.as_str(), item.reference.version.as_str()),
				item,
			)
		})
		.collect();
	let mut approvals = vec![];
	let mut installations = vec![];
	let mut satisfied = BTreeSet::new();
	let mut activated = BTreeSet::new();
	for item in &submission.items {
		let key = (item.reference.id.as_str(), item.reference.version.as_str());
		let reviewed = by_reference.get(&key).ok_or_else(|| {
			Error::Conflict("APPROVAL_REVIEW_STALE: item left the closure".into())
		})?;
		if reviewed.installation.is_some() {
			return Err(Error::Conflict(
				"APPROVAL_REVIEW_STALE: an installed copy is submitted as activation".into(),
			));
		}
		if !reviewed.can_approve {
			return Err(Error::Forbidden);
		}
		if reviewed.digest.as_deref() != Some(item.digest.as_str())
			|| reviewed.expected_revision != item.expected_revision
		{
			return Err(Error::Conflict(
				"APPROVAL_REVIEW_STALE: item revision or digest changed".into(),
			));
		}
		match reviewed.status {
			ReviewStatus::Approvable => {}
			ReviewStatus::Approved => {
				if item.restriction.is_none() || item.restriction == reviewed.restriction {
					return Err(Error::Invalid("APPROVAL_REVIEW_NO_CHANGE".into()));
				}
			}
			ReviewStatus::NeedsOtherApprover => return Err(Error::Forbidden),
			_ => {
				return Err(Error::Conflict(
					"APPROVAL_REVIEW_STALE: item is not approvable".into(),
				));
			}
		}
		validate_revision(item.expected_revision)?;
		if let Some(restriction) = &item.restriction {
			restriction.validate()?;
		}
		if !satisfied.insert((item.reference.id.clone(), item.reference.version.clone())) {
			return Err(Error::Invalid("APPROVAL_REVIEW_DUPLICATE".into()));
		}
		approvals.push(Approval {
			entry: item.reference.clone(),
			enabled: true,
			expected_revision: item.expected_revision,
			groups: if item.reference == review.root && reviewed.kind == "agent" {
				submission.root_groups.clone()
			} else {
				None
			},
			restriction: item.restriction.clone(),
		});
	}
	for activation in &submission.installations {
		let reviewed = review
			.items
			.iter()
			.find(|item| {
				item.installation.as_ref().is_some_and(|installed| {
					installed.installation == activation.installation
						&& installed.revision == activation.revision
				})
			})
			.ok_or_else(|| {
				Error::Conflict("APPROVAL_REVIEW_STALE: installation left the closure".into())
			})?;
		if !reviewed.can_approve {
			return Err(Error::Forbidden);
		}
		let installed = reviewed
			.installation
			.as_ref()
			.ok_or_else(|| Error::Conflict("APPROVAL_REVIEW_STALE: not an installation".into()))?;
		if installed.digest != activation.digest
			|| installed.expected_activation_revision != activation.expected_activation_revision
		{
			return Err(Error::Conflict(
				"APPROVAL_REVIEW_STALE: installation revision or digest changed".into(),
			));
		}
		match reviewed.status {
			ReviewStatus::NeedsMarketplaceActivation => {}
			ReviewStatus::Approved => {
				if activation.restriction.is_none()
					|| activation.restriction == reviewed.restriction
				{
					return Err(Error::Invalid("APPROVAL_REVIEW_NO_CHANGE".into()));
				}
			}
			ReviewStatus::NeedsOtherApprover => return Err(Error::Forbidden),
			_ => {
				return Err(Error::Conflict(
					"APPROVAL_REVIEW_STALE: installation is not activatable".into(),
				));
			}
		}
		if let Some(restriction) = &activation.restriction {
			restriction.validate()?;
		}
		if !satisfied.insert((
			reviewed.reference.id.clone(),
			reviewed.reference.version.clone(),
		)) {
			return Err(Error::Invalid("APPROVAL_REVIEW_DUPLICATE".into()));
		}
		activated.insert((
			reviewed.reference.id.clone(),
			reviewed.reference.version.clone(),
		));
		activated.insert((
			reviewed.reference.id.clone(),
			reviewed.reference.version.clone(),
		));
		installations.push(activation.clone());
	}
	Ok(ReviewPlan {
		approvals,
		installations,
		satisfied_by_activation: activated.clone(),
		activated,
	})
}

/// Turn a validated activation plan into the existing Marketplace approval-set
/// input, with the review Node and each installation's restriction.
pub fn approval_set(
	tenant: &str,
	node: &NodeName,
	review: &ApprovalReview,
	plan: &ReviewPlan,
) -> Result<crate::marketplace::operations::approval_set::ApprovalSet> {
	let mut selections = vec![];
	let mut approvals = vec![];
	for activation in &plan.installations {
		let reviewed = review
			.items
			.iter()
			.find(|item| {
				item.installation.as_ref().is_some_and(|installed| {
					installed.installation == activation.installation
						&& installed.revision == activation.revision
				})
			})
			.ok_or_else(|| Error::Conflict("APPROVAL_REVIEW_STALE: installation changed".into()))?;
		selections.push(PendingSelection {
			installation: activation.installation.clone(),
			revision: activation.revision,
			digest: activation.digest.clone(),
			expected_activation_revision: activation.expected_activation_revision,
			restriction: activation.restriction.clone(),
		});
		approvals.push(ApprovalSelection {
			reference: reviewed.reference.clone(),
			expected_catalog_revision: reviewed.expected_revision,
			restriction: activation.restriction.clone(),
		});
	}
	Ok(crate::marketplace::operations::approval_set::ApprovalSet {
		tenant: tenant.into(),
		node: Some(node.clone()),
		installations: selections,
		approvals,
	})
}

#[cfg(test)]
mod tests;
