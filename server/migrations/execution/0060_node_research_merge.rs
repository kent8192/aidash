// reinhardt-migration-source: 1
// Join Node lifetime cancellation with Research and approval history.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0060_node_research_merge", "execution")
		.add_dependency("execution", "0058_node_lifetime_merge")
		.add_dependency("execution", "0059_research_history_merge")
		.state_only(true)
}
