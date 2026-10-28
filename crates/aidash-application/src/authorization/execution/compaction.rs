//! A Run starts only when every compaction Decider its Agent's pinned Bindings
//! name holds its Catalog Approval on the executing Node (#540). A Run that is
//! already running keeps the runtime pause (`DeciderUnapproved`) when the
//! approval is later withdrawn.
use crate::{Error, Result};
use aidash_domain::registry::{
	Entry,
	bindings::{BindingSnapshot, ResolvedBinding, defaults::is_compaction_decider},
	rules::digest,
};

/// Stable error code of a start refused for an unapproved compaction Decider.
/// The message is `COMPACTION_DECIDER_UNAPPROVED: <id>@<version>`.
pub const UNAPPROVED: &str = "COMPACTION_DECIDER_UNAPPROVED";

/// The compaction Deciders the Run would actually use: default or explicit,
/// not excluded. Generated and RequiredHome chains exclude the implicit
/// default and keep their runtime check.
pub fn bound(snapshot: &BindingSnapshot) -> impl Iterator<Item = &ResolvedBinding> {
	snapshot.bindings.iter().filter(|binding| {
		binding.excluded_reason.is_none() && is_compaction_decider(&binding.definition)
	})
}

/// Judge the Decider's current Catalog entry (read with `decision.invoke` on
/// this Node) against the pinned binding, like the runtime check does: a
/// denied or missing entry, or a changed definition, is unapproved.
pub fn require(binding: &ResolvedBinding, current: Result<Entry>) -> Result<()> {
	let unapproved = || {
		Error::Conflict(format!(
			"{UNAPPROVED}: {}@{}",
			binding.identity.id, binding.identity.version
		))
	};
	match current {
		Ok(entry) if digest(&serde_json::to_value(&entry)?) == binding.digest => Ok(()),
		Ok(_) | Err(Error::Forbidden | Error::NotFound(_)) => Err(unapproved()),
		Err(error) => Err(error),
	}
}
