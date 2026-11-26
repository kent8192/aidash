// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0074_thread_scratch_wipes", "workspaces")
		.add_dependency("workspaces", "0073_scratch_wipe_receipts")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "threads".into(),
			column: ColumnDefinition::new("scratch_wipe_requested", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".into())),
			mysql_options: None,
		})
}
