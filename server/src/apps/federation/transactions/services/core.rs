#[path = "authority.rs"]
pub mod authority;
#[path = "coordinator.rs"]
pub mod coordinator;
#[path = "fault.rs"]
pub mod fault;
#[path = "gate.rs"]
pub mod gate;
#[path = "participant.rs"]
pub mod participant;

/// Native callers assemble registry validation through the shared bootstrap.
pub fn validate(manifest: &Manifest) -> crate::Result<()> {
	aidash_application::transactions::validate(&crate::bootstrap::registry_validation(), manifest)
		.map_err(Into::into)
}

pub use crate::apps::federation::transactions::serializers::contracts::{
	Isolation, LocalStatus, Manifest, Mutation, Participant, Status, Vote,
};
