// reinhardt-migration-source: 1
// Scratch Space Runs are independent. Remove the queue notification and mark.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0180_unorder_runs", "execution")
		.add_dependency("execution", "0179_drive_tool_contract")
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0180_unorder_runs.sql").into(),
			reverse_sql: Some(include_str!("sql/forward/0153_area_queue_sleep.sql").into()),
		})
		.add_operation(Operation::DropColumn {
			table: "core_runs".into(),
			column: "asleep".into(),
			old_definition: Some(
				ColumnDefinition::new("asleep", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".into())),
			),
		})
}
