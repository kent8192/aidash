// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0027_task_journal_range", "workspaces")
		.add_dependency("workspaces", "0026_scratch_binding_indexes")
		.add_operation(Operation::AddColumn {
			table: "tasks".into(),
			column: ColumnDefinition::new("journal_range", FieldType::Jsonb),
			mysql_options: None,
		})
}
