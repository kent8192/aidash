// reinhardt-migration-source: 1
// A Run a Duty started may set up Duties like any other (#1012), so nothing
// marks the execution authority of such a Run any more (0081).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0097_unscheduled_executions", "identity")
		.add_dependency("identity", "0096_tenant_execution_settings_state")
		.add_operation(Operation::DropColumn {
			table: "authorization_execution".into(),
			column: "scheduled".into(),
			old_definition: Some(
				ColumnDefinition::new("scheduled", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
			),
		})
		.atomic(true)
}
