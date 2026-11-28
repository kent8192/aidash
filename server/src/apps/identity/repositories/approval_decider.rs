//! Identity of Aidash's code-defined Auto Approval judge.
use aidash_application::registry::system::{APPROVAL_DECIDER_ID, APPROVAL_DECIDER_VERSION};
use aidash_domain::registry::EntityRef;

pub(crate) fn built_in() -> EntityRef {
	EntityRef {
		id: APPROVAL_DECIDER_ID.into(),
		version: APPROVAL_DECIDER_VERSION.into(),
	}
}

/// Availability reported when editing an Approval Mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Availability {
	Usable,
	Missing,
}
