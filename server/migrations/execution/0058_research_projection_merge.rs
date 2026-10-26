// reinhardt-migration-source: 1
// Join the Research/approval and projection evidence histories without rewriting them.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0058_research_projection_merge", "execution")
		.add_dependency("execution", "0057_approval_observations_state")
		.add_dependency("execution", "0051_compaction_provider_dispatch")
		.state_only(true)
}
