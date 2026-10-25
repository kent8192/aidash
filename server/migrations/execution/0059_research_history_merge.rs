// reinhardt-migration-source: 1
// Join durable Research results with the approval and projection evidence history.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0059_research_history_merge", "execution")
		.add_dependency("execution", "0058_research_projection_merge")
		.add_dependency("execution", "0051_research_journal_results_state")
		.state_only(true)
}
