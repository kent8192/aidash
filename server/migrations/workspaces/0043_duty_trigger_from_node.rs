// reinhardt-migration-source: 1
// A `node_event` Duty Trigger may name the only Node it takes events from
// (#1013). The column is in the ORM model; 0044 lets the kind check accept
// the new kind and keeps the column to it.
use reinhardt::db::migrations::FieldType;
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0043_duty_trigger_from_node", "workspaces")
		.add_dependency("workspaces", "0038_task_duty")
		.add_operation(Operation::AddColumn {
			table: "duty_triggers".into(),
			column: ColumnDefinition::new("from_node", FieldType::Text)
				.with_not_null(false)
				.with_unique(false)
				.with_primary_key(false)
				.with_auto_increment(false)
				.with_default(None)
				.with_generated(None)
				.with_domain_option(None),
			mysql_options: None,
		})
		.atomic(true)
}
