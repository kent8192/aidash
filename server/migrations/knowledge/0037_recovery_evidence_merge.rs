// reinhardt-migration-source: 1
// Join recovery eligibility and attempt evidence without rewriting either history.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0037_recovery_evidence_merge", "knowledge")
		.add_dependency("knowledge", "0036_memory_recovery_eligibility")
		.add_dependency("knowledge", "0036_memory_attempt_evidence")
		.state_only(true)
}
