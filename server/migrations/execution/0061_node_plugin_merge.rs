// reinhardt-migration-source: 1
// Join Node lifetime cancellation and MCP continuation histories.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0061_node_plugin_merge", "execution")
		.add_dependency("execution", "0060_node_research_merge")
		.add_dependency("execution", "0060_plugin_research_merge")
		.state_only(true)
}
