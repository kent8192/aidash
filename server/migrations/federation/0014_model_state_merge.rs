// reinhardt-migration-source: 1
//! Join the baseline model state and the later federation database history.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0014_model_state_merge", "federation")
		.add_dependency("federation", "0007_model_state")
		.add_dependency("federation", "0013_registered_preview_origin")
		.state_only(true)
}
