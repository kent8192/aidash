// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0171_outside_versions", "execution")
		.add_dependency("execution", "0170_goal_judge_calls")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "task_executions".into(),
			column: ColumnDefinition::new("outside_versions", FieldType::Jsonb),
			mysql_options: None,
		})
}
