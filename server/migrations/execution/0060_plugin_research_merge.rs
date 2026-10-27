// reinhardt-migration-source: 1
// Join MCP continuations with Research and approval history without changing either.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0060_plugin_research_merge", "execution")
		.add_dependency("execution", "0052_merge_mcp_and_usage_evidence")
		.add_dependency("execution", "0059_research_history_merge")
		.state_only(true)
}
