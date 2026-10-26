// reinhardt-migration-source: 1
// Join Context Source and Marketplace histories without rewriting either branch.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0048_context_marketplace_merge", "registry")
		.add_dependency("registry", "0046_context_projection_merge")
		.add_dependency("registry", "0047_marketplace_recall_constraints")
		.state_only(true)
}
