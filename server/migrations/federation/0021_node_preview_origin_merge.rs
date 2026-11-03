// reinhardt-migration-source: 1
// Join Node registration recovery and registered preview origin histories.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0021_node_preview_origin_merge", "federation")
		.add_dependency("federation", "0020_node_registration_recovery_ownership")
		.add_dependency("federation", "0013_registered_preview_origin")
		.state_only(true)
}
