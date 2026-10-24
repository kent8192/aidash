// reinhardt-migration-source: 1
// Source identity remains, with insertion-time same-Tenant KeyShare existence.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0054_session_area_lifetime", "execution")
		.add_dependency("execution", "0050_runner_allocation_journals")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_runs".into(),
			constraint: Constraint::ForeignKey {
				name: "core_runs_area_id_fkey".into(),
				columns: vec!["tenant".into(), "area_id".into()],
				referenced_table: "core_areas".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
}
