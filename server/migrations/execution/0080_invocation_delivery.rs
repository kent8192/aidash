// reinhardt-migration-source: 1
//! Durable delivery state for retries and reconciliation of tool calls.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0080_invocation_delivery", "execution")
		.add_dependency("execution", "0079_run_safety_stops_state")
		.add_operation(Operation::AddColumn {
			table: "invocations".into(),
			column: ColumnDefinition::new("delivery_failures", FieldType::Integer)
				.with_not_null(true)
				.with_default(Some("0".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "invocations".into(),
			column: ColumnDefinition::new("delivery_pending", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "invocations".into(),
			column: ColumnDefinition::new("uncertain_reason", FieldType::Text),
			mysql_options: None,
		})
}
