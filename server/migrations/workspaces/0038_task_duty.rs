// reinhardt-migration-source: 1
// Each Task records the Duty it works for, once, when it is created: a Duty's
// own Task from the Duty, any other from the Task of the Run that created it
// (#1012). Budgets charge a Duty's spend through it without walking Task
// trees. It is database-only so the ORM model, which never sets it, keeps
// its shape; the writers use Reinhardt Query.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0038_task_duty", "workspaces")
		.add_dependency("workspaces", "0037_duty_triggers_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "tasks".into(),
			column: ColumnDefinition::new("duty_id", FieldType::Uuid),
			mysql_options: None,
		})
}
