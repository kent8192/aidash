// reinhardt-migration-source: 1
// Retain Node cancellation and Session Area lifetime histories without rewriting them.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0095_node_lifetime_merge", "execution")
		.add_dependency("execution", "0091_node_cancellation_jobs_state")
		.add_dependency("execution", "0094_session_area_lifetime_state")
		.state_only(true)
}
