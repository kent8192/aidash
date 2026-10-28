// reinhardt-migration-source: 1
// Join Agent Plugin and Marketplace histories without rewriting either branch.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0048_plugin_marketplace_merge", "registry")
		.add_dependency("registry", "0046_merge_agent_plugin_and_projection")
		.add_dependency("registry", "0047_marketplace_recall_constraints")
		.state_only(true)
}
