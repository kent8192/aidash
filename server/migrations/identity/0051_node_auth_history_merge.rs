// reinhardt-migration-source: 1
// Retain independent Auth bindings, Catalog Approval Requests, remote
// receipts and task source lifetimes.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0051_node_auth_history_merge", "identity")
		.add_dependency("identity", "0044_catalog_approval_requests_state")
		.add_dependency("identity", "0050_remote_terminal_receipts_state")
		.state_only(true)
}
