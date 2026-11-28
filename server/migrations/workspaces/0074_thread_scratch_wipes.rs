// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0074_thread_scratch_wipes", "workspaces")
		.add_dependency("workspaces", "0073_default_scratch_index")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "scratch_spaces".into(),
			column: ColumnDefinition::new("wipe_completed_generation", FieldType::BigInteger)
				.with_not_null(true)
				.with_default(Some("0".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "threads".into(),
			column: ColumnDefinition::new("scratch_wipe_requested", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".into())),
			mysql_options: None,
		})
}
