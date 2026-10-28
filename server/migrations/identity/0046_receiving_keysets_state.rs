// reinhardt-migration-source: 1
//! Receiver-local trust changes independently of authorization (ADR 0084).
//! Empty since the Peer Graph Grant was removed (#486); see 0045.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0046_receiving_keysets_state", "identity")
		.add_dependency("identity", "0045_receiving_keysets")
		.state_only(true)
		.atomic(true)
}
