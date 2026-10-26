// reinhardt-migration-source: 1
// Join Harness Contract usage evidence and MCP continuation histories.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0062_harness_plugin_merge", "execution")
		.add_dependency("execution", "0061_harness_contract_state")
		.add_dependency("execution", "0060_plugin_research_merge")
		.state_only(true)
}
