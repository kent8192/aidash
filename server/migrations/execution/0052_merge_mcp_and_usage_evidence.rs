// reinhardt-migration-source: 1
// Join independent MCP kind and provider evidence histories without rewriting either.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0052_merge_mcp_and_usage_evidence", "execution")
		.add_dependency("execution", "0051_compaction_provider_dispatch")
		.add_dependency("execution", "0051_mcp_elicitation_state")
}
