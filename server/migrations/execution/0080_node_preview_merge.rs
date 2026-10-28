// reinhardt-migration-source: 1
// Join Node lifecycle with the federated Live preview and Safety Stop histories.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0080_node_preview_merge", "execution")
		.add_dependency("execution", "0061_node_plugin_merge")
		.add_dependency("execution", "0079_run_safety_stops_state")
		.state_only(true)
}
