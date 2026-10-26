// reinhardt-migration-source: 1
// Join Harness Contract validation with Agent Plugin and Context Source history.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0050_harness_plugin_merge", "registry")
		.add_dependency("registry", "0049_harness_contract")
		.add_dependency("registry", "0049_plugin_context_merge")
		.state_only(true)
}
