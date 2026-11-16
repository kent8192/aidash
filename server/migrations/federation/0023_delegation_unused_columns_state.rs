// reinhardt-migration-source: 1
//! Model state for `0022_delegation_unused_columns` (#373).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0023_delegation_unused_columns_state", "federation")
		.add_dependency("federation", "0022_delegation_unused_columns")
		.state_only(true)
		.add_operation(Operation::DropColumn {
			table: "delegations".into(),
			column: "binding_snapshot".into(),
			old_definition: Some(ColumnDefinition::new("binding_snapshot", FieldType::Jsonb)),
		})
		.add_operation(Operation::DropColumn {
			table: "delegations".into(),
			column: "human_requests".into(),
			old_definition: Some(
				ColumnDefinition::new("human_requests", FieldType::Jsonb).with_not_null(true),
			),
		})
		.add_operation(Operation::DropColumn {
			table: "delegations".into(),
			column: "next_attempt_at".into(),
			old_definition: Some(
				ColumnDefinition::new("next_attempt_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("now()".into())),
			),
		})
		.add_operation(Operation::DropColumn {
			table: "delegations".into(),
			column: "delivered".into(),
			old_definition: Some(
				ColumnDefinition::new("delivered", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".into())),
			),
		})
}
