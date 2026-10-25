// reinhardt-migration-source: 1
// Join Home ledger eligibility and remote-read provenance without changing either.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0038_recovery_lifetime_merge", "knowledge")
		.add_dependency("knowledge", "0037_recovery_evidence_merge")
		.add_dependency("knowledge", "0037_remote_read_source_lifetime")
		.state_only(true)
}
