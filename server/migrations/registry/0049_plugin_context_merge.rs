// reinhardt-migration-source: 1
// Join Agent Plugin and Context Source histories after their Marketplace merges.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0049_plugin_context_merge", "registry")
		.add_dependency("registry", "0048_plugin_marketplace_merge")
		.add_dependency("registry", "0048_context_marketplace_merge")
		.state_only(true)
}
