// reinhardt-migration-source: 1
// Join Node lifecycle and federated Live preview histories.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0075_node_preview_merge", "execution")
		.add_dependency("execution", "0061_node_plugin_merge")
		.add_dependency("execution", "0074_federated_preview_authority")
		.state_only(true)
}
