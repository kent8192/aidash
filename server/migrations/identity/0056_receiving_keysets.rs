// reinhardt-migration-source: 1
//! Receiver-local trust changes independently of authorization (ADR 0084).
//! The Peer Graph Grant keyset this added was removed with the grant (#486);
//! the migration stays as an empty step so later identity history keeps its
//! dependencies.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0056_receiving_keysets", "identity")
		.add_dependency("identity", "0055_node_administration_target_state")
		.database_only(true)
		.atomic(true)
}
