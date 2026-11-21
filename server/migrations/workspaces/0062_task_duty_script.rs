// reinhardt-migration-source: 1
// A Task a Duty started to run its Router or Watcher keeps that it runs a
// script (#1011). Its `duty_script_runs` row goes with the Duty; its Run must
// then fail rather than ask a model on the Task's stub description. It is
// database-only, like `tasks.duty_id`: the ORM model never sets it and its
// writer and reader use Reinhardt Query.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0062_task_duty_script", "workspaces")
		.add_dependency("workspaces", "0061_duty_template_check")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "tasks".into(),
			column: ColumnDefinition::new("duty_script", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".to_owned())),
			mysql_options: None,
		})
}
