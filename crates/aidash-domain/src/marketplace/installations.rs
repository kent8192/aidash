//! Deterministic staging and equivalence of immutable installation revisions.
use super::definitions::{content, key};
use super::*;
use crate::registry::{EntityRef, Entry, Projection};
use serde_json::Value;
use uuid::Uuid;

pub struct StagedRevision {
	pub installation: Installation,
	pub revision: Revision,
	pub provenance: BTreeSet<ConsentEdge>,
}
pub fn stage_revision(
	mut installation: Installation,
	source: &Version,
	config: Value,
	resolved: (Entry, Vec<EntityRef>, Vec<DependencyBinding>),
	entry_id: Uuid,
) -> StagedRevision {
	let (mut entry, dependencies, bindings) = resolved;
	installation.latest_revision += 1;
	let revision_number = installation.latest_revision;
	entry.id = format!("mkt-{}", entry_id.simple());
	entry.version = "1.0.0".into();
	entry.installation = Some(Projection {
		contract: 1,
		tenant: installation.tenant.clone(),
		installation: installation.id.clone(),
		revision: revision_number,
	});
	let revision = Revision {
		installation: installation.id.clone(),
		tenant: installation.tenant.clone(),
		revision: revision_number,
		digest: key(&entry),
		entry,
		config,
		dependencies,
		bindings,
		source: source.clone(),
	};
	let mut provenance = source.lineage.clone();
	for grant in [SourceGrant::Consent, SourceGrant::Audience] {
		provenance.insert(ConsentEdge {
			source: source.key.clone(),
			redistributor: installation.tenant.clone(),
			grant,
		});
	}
	StagedRevision {
		installation,
		revision,
		provenance,
	}
}
pub fn same(
	current: &Revision,
	candidate: &Entry,
	config: &Value,
	bindings: &[DependencyBinding],
) -> bool {
	content(&current.entry) == content(candidate)
		&& current.config == *config
		&& key(&current.bindings) == key(&bindings)
}

pub fn compatible(state: &Compatibility) -> bool {
	state.enabled && state.contract == 1
}
/// Revocation retains the selected pointer, preventing fallback to an older grant.
pub fn activation(mut installation: Installation, revision: i64, enabled: bool) -> Installation {
	if enabled {
		installation.active_revision = Some(revision);
	}
	installation.activation_revision += 1;
	installation
}
/// Only Marketplace activation can manage approvals of projected definitions.
pub fn catalog_update_allowed(entry: &Entry) -> bool {
	entry.installation.is_none()
}
