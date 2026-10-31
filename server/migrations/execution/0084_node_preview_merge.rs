// reinhardt-migration-source: 1
// Join Node lifecycle with the federated Live preview, Safety Stop, Lease Recovery and Harness Contract histories.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0084_node_preview_merge", "execution")
		.add_dependency("execution", "0061_node_plugin_merge")
		.add_dependency("execution", "0083_harness_contract_state")
		.state_only(true)
}
