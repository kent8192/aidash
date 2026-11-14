// reinhardt-migration-source: 1
// Mark the execution authority of a Run a Schedule started, and of every Run it
// delegates to, so none of them registers or changes Schedules (#557).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0074_scheduled_executions", "identity")
		.add_dependency("identity", "0073_schedule_member_policy")
		.add_operation(Operation::AddColumn {
			table: "authorization_execution".into(),
			column: ColumnDefinition::new("scheduled", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".to_owned())),
			mysql_options: None,
		})
		.atomic(true)
}
