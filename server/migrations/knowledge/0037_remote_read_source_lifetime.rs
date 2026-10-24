// reinhardt-migration-source: 1
// A remote read owns content-free provenance, not the source Unit's lifetime.
// The sole record writer retains same-Tenant existence with FOR KEY SHARE.
// Logical state already removed this scalar source FK in knowledge/0023.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0037_remote_read_source_lifetime", "knowledge")
		.add_dependency("knowledge", "0036_memory_attempt_evidence")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "memory_remote_reads".into(),
			constraint: Constraint::ForeignKey {
				name: "memory_remote_read_unit".into(),
				columns: vec!["tenant".into(), "unit_id".into()],
				referenced_table: "memory_units".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::Restrict,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
}
