// reinhardt-migration-source: 1
// Retain independent Auth bindings, remote receipts and task source lifetimes.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0051_node_auth_history_merge", "identity")
		.add_dependency("identity", "0040_common_auth_origin_binding")
		.add_dependency("identity", "0050_remote_terminal_receipts_state")
		.state_only(true)
}
