// reinhardt-migration-source: 1
// Both contract extensions remain intact; only their migration leaves are joined.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0046_merge_agent_plugin_and_projection", "registry")
		.add_dependency("registry", "0045_agent_plugin_settings")
		.add_dependency("registry", "0045_projection_measurement_config")
}
