// reinhardt-migration-source: 1
// Join the Context Source and projection configuration histories without rewriting them.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0046_context_projection_merge", "registry")
		.add_dependency("registry", "0045_context_sources")
		.add_dependency("registry", "0045_projection_measurement_config")
		.state_only(true)
}
