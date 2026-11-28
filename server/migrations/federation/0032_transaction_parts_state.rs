// reinhardt-migration-source: 1
//! Model state for `0031_transaction_parts` (#1107).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0032_transaction_parts_state", "federation")
		.add_dependency("federation", "0031_transaction_parts")
		.state_only(true)
		.add_operation(Operation::AddColumn {
			table: "atomic_coordinators".into(),
			column: ColumnDefinition::new("seed", FieldType::Text).with_not_null(true),
			mysql_options: None,
		})
}
