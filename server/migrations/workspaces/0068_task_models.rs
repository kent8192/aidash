// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0068_task_models", "workspaces")
		.add_dependency("workspaces", "0067_task_duty_start")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "tasks".into(),
			column: ColumnDefinition::new("task_model", FieldType::Jsonb),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "duties".into(),
			column: ColumnDefinition::new("task_model", FieldType::Jsonb),
			mysql_options: None,
		})
}
