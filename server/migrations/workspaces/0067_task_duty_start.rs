// reinhardt-migration-source: 1
// Record the Duty that started a Task automatically (#990). Reinhardt Query
// backfills automatic ticks and script Tasks, including their stored events.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0067_task_duty_start", "workspaces")
		.add_dependency("workspaces", "0066_deleted_duty_flow_ancestry")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "tasks".into(),
			column: ColumnDefinition::new("started_by_duty", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0067_task_duty_start.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0067_task_duty_start.sql").into()),
		})
}
