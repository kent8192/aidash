// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0070_task_goals", "workspaces")
		.add_dependency("workspaces", "0067_task_duty_start")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "tasks".into(),
			column: ColumnDefinition::new("goal", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "tasks".into(),
			column: ColumnDefinition::new("goal_history", FieldType::Jsonb),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "tasks".into(),
			column: ColumnDefinition::new("goal_verdicts", FieldType::Jsonb),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "duties".into(),
			column: ColumnDefinition::new("goal", FieldType::Text),
			mysql_options: None,
		})
}
